use std::sync::Arc;

use parking_lot::RwLock;

use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::query_registry::CancelToken;
use crate::executor::streaming::slot::SlotLayout;
use crate::storage::QueryStorage;
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::{EdgeDirection, Value};

use super::super::runtime::ExecutionRuntime;
use super::spec::GraphSpec;
use super::visited_set::VisitedSet;

mod common;
mod expand;
mod subgraph;
mod traverse;

pub(super) struct ExpandCtx<'a> {
    pub(super) space_name: &'a str,
    pub(super) edge_types: &'a [String],
    pub(super) direction: EdgeDirection,
    pub(super) filter_expr: &'a Option<Expression>,
    pub(super) col_names_template: Vec<String>,
    pub(super) cancel_token: Option<CancelToken>,
}

#[derive(Debug)]
pub enum GraphOperatorKind {
    Expand {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
    },
    ExpandAll {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
        col_names: Vec<String>,
        src_vids: Vec<Value>,
        step_limit: u32,
        count_only: bool,
        emit_raw_ids: bool,
        lightweight_source: bool,
    },
    Traverse {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        filter_expr: Option<Expression>,
        visited: VisitedSet,
    },
    TraverseAll {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        filter_expr: Option<Expression>,
        visited: VisitedSet,
    },
    BiExpand {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
    },
    BiTraverse {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        visited: VisitedSet,
    },
    Subgraph {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        steps: u32,
        direction: EdgeDirection,
        edge_types: Vec<String>,
    },
}

/// Graph operator.
///
/// Wraps [`GraphOperatorKind`] with the runtime context injected at `open()`.
/// Lifecycle state is owned exclusively by the executor; operators never
/// write it.
#[derive(Debug)]
pub struct GraphOperator {
    pub kind: GraphOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl GraphOperator {
    pub fn bind_runtime(&mut self, runtime: &Arc<ExecutionRuntime>) {
        let storage = runtime.storage.clone();
        let space_name = runtime.query_id().space_name.unwrap_or_default();
        self.runtime = Some(Arc::clone(runtime));
        match &mut self.kind {
            GraphOperatorKind::Expand {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | GraphOperatorKind::ExpandAll {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | GraphOperatorKind::Traverse {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | GraphOperatorKind::TraverseAll {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | GraphOperatorKind::BiExpand {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | GraphOperatorKind::BiTraverse {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | GraphOperatorKind::Subgraph {
                storage: target_storage,
                space_name: target_space,
                ..
            } => {
                *target_storage = storage;
                *target_space = space_name;
            }
        }
    }

    pub fn from_spec(
        spec: &GraphSpec,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        output_layout: Arc<SlotLayout>,
    ) -> Self {
        let kind = match spec {
            GraphSpec::Expand {
                edge_types,
                direction,
                filter_expr,
                ..
            } => GraphOperatorKind::Expand {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                filter_expr: filter_expr.clone(),
            },
            GraphSpec::ExpandAll {
                edge_types,
                direction,
                filter_expr,
                col_names,
                src_vids,
                step_limit,
                count_only,
                emit_raw_ids,
                lightweight_source,
            } => GraphOperatorKind::ExpandAll {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                filter_expr: filter_expr.clone(),
                col_names: col_names.clone(),
                src_vids: src_vids.clone(),
                step_limit: *step_limit,
                count_only: *count_only,
                emit_raw_ids: *emit_raw_ids,
                lightweight_source: *lightweight_source,
            },
            GraphSpec::Traverse {
                edge_types,
                direction,
                min_depth,
                max_depth,
                filter_expr,
            } => GraphOperatorKind::Traverse {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                min_depth: *min_depth,
                max_depth: *max_depth,
                filter_expr: filter_expr.clone(),
                visited: VisitedSet::new(),
            },
            GraphSpec::BiExpand {
                edge_types,
                direction,
            } => GraphOperatorKind::BiExpand {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
            },
            GraphSpec::BiTraverse {
                edge_types,
                direction,
                min_depth,
                max_depth,
            } => GraphOperatorKind::BiTraverse {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                min_depth: *min_depth,
                max_depth: *max_depth,
                visited: VisitedSet::new(),
            },
        };
        Self::new(kind, output_layout)
    }

    pub fn new(kind: GraphOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
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
            GraphOperatorKind::Expand { .. }
            | GraphOperatorKind::ExpandAll { .. }
            | GraphOperatorKind::Traverse { .. }
            | GraphOperatorKind::TraverseAll { .. }
            | GraphOperatorKind::BiExpand { .. }
            | GraphOperatorKind::BiTraverse { .. }
            | GraphOperatorKind::Subgraph { .. } => {
                input.open()?;
                Ok(())
            }
        }
    }

    pub fn next(&mut self, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
        let op: &mut GraphOperator = self;
        if matches!(&op.kind, GraphOperatorKind::Expand { .. }) {
            return expand::handle(op, input);
        }
        if matches!(&op.kind, GraphOperatorKind::ExpandAll { .. }) {
            return expand::handle_all(op, input);
        }
        if matches!(&op.kind, GraphOperatorKind::Traverse { .. }) {
            return traverse::handle_traverse(op, input);
        }
        if matches!(&op.kind, GraphOperatorKind::TraverseAll { .. }) {
            return traverse::handle_traverse_all(op, input);
        }
        if matches!(&op.kind, GraphOperatorKind::BiExpand { .. }) {
            return traverse::handle_bi_expand(op, input);
        }
        if matches!(&op.kind, GraphOperatorKind::BiTraverse { .. }) {
            return traverse::handle_bi_traverse(op, input);
        }
        if matches!(&op.kind, GraphOperatorKind::Subgraph { .. }) {
            return subgraph::handle(op, input);
        }
        unreachable!("graph_operator::next called for an unknown kind")
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    /// Reset per-run graph state (visited sets) and rewind the input so the
    /// graph operator re-produces the same traversal.
    pub fn reset(&mut self, input: &mut StreamingExecutor) -> Result<bool, QueryError> {
        match &mut self.kind {
            GraphOperatorKind::Traverse { visited, .. }
            | GraphOperatorKind::TraverseAll { visited, .. }
            | GraphOperatorKind::BiTraverse { visited, .. } => {
                *visited = VisitedSet::new();
            }
            GraphOperatorKind::Expand { .. }
            | GraphOperatorKind::ExpandAll { .. }
            | GraphOperatorKind::BiExpand { .. }
            | GraphOperatorKind::Subgraph { .. } => {}
        }
        input.reset()?;
        Ok(false)
    }
}
