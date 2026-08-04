use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::storage::QueryStorage;

use super::super::runtime::ExecutionRuntime;
use super::spec::GraphSpec;
use super::visited_set::VisitedSet;

mod common;
mod expand;
mod subgraph;
mod traverse;

pub(super) struct GraphCtx<'a> {
    pub(super) storage: &'a Option<Arc<RwLock<dyn QueryStorage>>>,
    pub(super) space_name: &'a str,
    pub(super) edge_types: &'a [String],
    pub(super) direction: EdgeDirection,
    pub(super) base: &'a mut OperatorBase,
    pub(super) input: &'a mut StreamingExecutor,
    pub(super) is_recursive: bool,
}

pub(super) struct ExpandCtx<'a> {
    pub(super) space_name: &'a str,
    pub(super) edge_types: &'a [String],
    pub(super) direction: EdgeDirection,
    pub(super) filter_expr: &'a Option<Expression>,
    pub(super) col_names_template: Vec<String>,
    pub(super) cancel_token: Option<Arc<AtomicBool>>,
}

#[derive(Debug)]
pub enum GraphOperator {
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

impl GraphOperator {
    pub fn bind_runtime(&mut self, runtime: &ExecutionRuntime) {
        let storage = runtime.storage.clone();
        let space_name = runtime.query_id().space_name.unwrap_or_default();
        match self {
            Self::Expand {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::ExpandAll {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::Traverse {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::TraverseAll {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::BiExpand {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::BiTraverse {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::Subgraph {
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
    ) -> Self {
        match spec {
            GraphSpec::Expand {
                edge_types,
                direction,
                filter_expr,
                ..
            } => Self::Expand {
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
            } => Self::ExpandAll {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                filter_expr: filter_expr.clone(),
                col_names: col_names.clone(),
                src_vids: src_vids.clone(),
                step_limit: *step_limit,
                count_only: *count_only,
            },
            GraphSpec::Traverse {
                edge_types,
                direction,
                min_depth,
                max_depth,
                filter_expr,
            } => Self::Traverse {
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
            } => Self::BiExpand {
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
            } => Self::BiTraverse {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                min_depth: *min_depth,
                max_depth: *max_depth,
                visited: VisitedSet::new(),
            },
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        match self {
            Self::Expand { .. }
            | Self::ExpandAll { .. }
            | Self::Traverse { .. }
            | Self::TraverseAll { .. }
            | Self::BiExpand { .. }
            | Self::BiTraverse { .. }
            | Self::Subgraph { .. } => {
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
            Self::Expand {
                storage,
                space_name,
                edge_types,
                direction,
                filter_expr,
            } => expand::handle(
                &*storage,
                &*space_name,
                &*edge_types,
                *direction,
                &*filter_expr,
                base,
                input,
            ),

            Self::ExpandAll {
                storage,
                space_name,
                edge_types,
                direction,
                filter_expr,
                col_names,
                src_vids,
                step_limit,
                count_only,
            } => expand::handle_all(
                &*filter_expr,
                col_names.clone(),
                src_vids.clone(),
                *step_limit,
                *count_only,
                &mut GraphCtx {
                    storage,
                    space_name,
                    edge_types,
                    direction: *direction,
                    base,
                    input,
                    is_recursive: false,
                },
            ),

            Self::Traverse {
                storage,
                space_name,
                edge_types,
                direction,
                min_depth,
                max_depth,
                visited,
                ..
            } => traverse::handle_traverse(
                *min_depth,
                *max_depth,
                visited,
                &mut GraphCtx {
                    storage,
                    space_name,
                    edge_types,
                    direction: *direction,
                    base,
                    input,
                    is_recursive: true,
                },
            ),

            Self::TraverseAll { .. } => traverse::handle_traverse_all(base, input),

            Self::BiExpand {
                storage,
                space_name,
                edge_types,
                ..
            } => traverse::handle_bi_expand(&*storage, &*space_name, &*edge_types, base, input),

            Self::BiTraverse {
                storage,
                space_name,
                edge_types,
                min_depth,
                max_depth,
                visited,
                ..
            } => traverse::handle_bi_traverse(
                *min_depth,
                *max_depth,
                visited,
                &mut GraphCtx {
                    storage,
                    space_name,
                    edge_types,
                    direction: EdgeDirection::Both,
                    base,
                    input,
                    is_recursive: true,
                },
            ),

            Self::Subgraph {
                storage,
                space_name,
                steps,
                direction,
                edge_types,
            } => subgraph::handle(
                &*storage,
                &*space_name,
                *steps,
                *direction,
                &*edge_types,
                base,
                input,
            ),
        }
    }

    pub fn stop(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        if base.lifecycle.can_close() {
            match self {
                Self::Expand { .. }
                | Self::ExpandAll { .. }
                | Self::Traverse { .. }
                | Self::TraverseAll { .. }
                | Self::BiExpand { .. }
                | Self::BiTraverse { .. }
                | Self::Subgraph { .. } => {
                    base.lifecycle.mark_stopped();
                }
            }
        }
        Ok(())
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        if base.lifecycle.can_close() {
            match self {
                Self::Expand { .. }
                | Self::ExpandAll { .. }
                | Self::Traverse { .. }
                | Self::TraverseAll { .. }
                | Self::BiExpand { .. }
                | Self::BiTraverse { .. }
                | Self::Subgraph { .. } => {
                    base.lifecycle.mark_closed();
                }
            }
        }
        Ok(())
    }
}
