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

mod all_paths;
mod common;
mod expand;
mod shortest_path;
mod subgraph;
mod traverse;

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
    ShortestPath {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
    },
    BFSShortest {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        allow_loops: bool,
        frontier: Vec<Vec<Value>>,
        visited: VisitedSet,
    },
    AllPaths {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: usize,
        max_depth: usize,
        acyclic: bool,
        limit: Option<usize>,
        offset: usize,
        filter: Option<Expression>,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
        all_paths: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
    },
    MultiShortestPath {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        target_vertices: Vec<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        left_vertex_column: String,
        right_vertex_column: String,
        single_shortest: bool,
        all_paths: Vec<Vec<Value>>,
        result_iter: Option<std::vec::IntoIter<Vec<Value>>>,
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
            | Self::ShortestPath {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::BFSShortest {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::AllPaths {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::MultiShortestPath {
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
            } => Self::ExpandAll {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                filter_expr: filter_expr.clone(),
                col_names: col_names.clone(),
                src_vids: src_vids.clone(),
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
            GraphSpec::ShortestPath {
                target_vertex,
                edge_types,
                direction,
                max_depth,
                start_vertices,
                target_vertices,
            } => Self::ShortestPath {
                storage: storage.clone(),
                space_name: space_name.clone(),
                target_vertex: target_vertex.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                max_depth: *max_depth,
                start_vertices: start_vertices.clone(),
                target_vertices: target_vertices.clone(),
            },
            GraphSpec::BFSShortest {
                target_vertex,
                edge_types,
                direction,
                max_depth,
                allow_loops,
            } => Self::BFSShortest {
                storage: storage.clone(),
                space_name: space_name.clone(),
                target_vertex: target_vertex.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                max_depth: *max_depth,
                allow_loops: *allow_loops,
                frontier: Vec::new(),
                visited: VisitedSet::new(),
            },
            GraphSpec::AllPaths {
                target_vertex,
                edge_types,
                direction,
                min_depth,
                max_depth,
                acyclic,
                limit,
                offset,
                filter,
                start_vertices,
                target_vertices,
            } => Self::AllPaths {
                storage: storage.clone(),
                space_name: space_name.clone(),
                target_vertex: target_vertex.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                min_depth: *min_depth,
                max_depth: *max_depth,
                acyclic: *acyclic,
                limit: *limit,
                offset: *offset,
                filter: filter.clone(),
                start_vertices: start_vertices.clone(),
                target_vertices: target_vertices.clone(),
                all_paths: Vec::new(),
                result_iter: None,
            },
            GraphSpec::MultiShortestPath {
                target_vertices,
                edge_types,
                direction,
                max_depth,
                left_vertex_column,
                right_vertex_column,
                single_shortest,
            } => Self::MultiShortestPath {
                storage,
                space_name,
                target_vertices: target_vertices.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                max_depth: *max_depth,
                left_vertex_column: left_vertex_column.clone(),
                right_vertex_column: right_vertex_column.clone(),
                single_shortest: *single_shortest,
                all_paths: Vec::new(),
                result_iter: None,
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
            | Self::ShortestPath { .. }
            | Self::BFSShortest { .. }
            | Self::AllPaths { .. }
            | Self::MultiShortestPath { .. }
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
            } => expand::handle_all(
                &*storage,
                &*space_name,
                &*edge_types,
                *direction,
                &*filter_expr,
                col_names.clone(),
                src_vids.clone(),
                base,
                input,
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
                &*storage,
                &*space_name,
                &*edge_types,
                *direction,
                *min_depth,
                *max_depth,
                visited,
                base,
                input,
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
                &*storage,
                &*space_name,
                &*edge_types,
                *min_depth,
                *max_depth,
                visited,
                base,
                input,
            ),

            Self::ShortestPath {
                storage,
                space_name,
                target_vertex,
                edge_types,
                direction,
                max_depth,
                start_vertices,
                target_vertices,
                ..
            } => shortest_path::handle_shortest_path(
                &*storage,
                &*space_name,
                &*target_vertex,
                &*edge_types,
                *direction,
                *max_depth,
                &*start_vertices,
                &*target_vertices,
                base,
                input,
            ),

            Self::BFSShortest {
                storage,
                space_name,
                edge_types,
                direction,
                max_depth,
                ..
            } => shortest_path::handle_bfs_shortest(
                &*storage,
                &*space_name,
                &*edge_types,
                *direction,
                *max_depth,
                base,
                input,
            ),

            Self::AllPaths {
                storage,
                space_name,
                target_vertex,
                edge_types,
                direction,
                min_depth,
                max_depth,
                acyclic,
                limit,
                offset,
                filter,
                start_vertices,
                target_vertices,
                ..
            } => all_paths::handle_all_paths(
                &*storage,
                &*space_name,
                &*target_vertex,
                &*edge_types,
                *direction,
                *min_depth,
                *max_depth,
                *acyclic,
                &*limit,
                *offset,
                &*filter,
                &*start_vertices,
                &*target_vertices,
                base,
                input,
            ),

            Self::MultiShortestPath {
                storage,
                space_name,
                target_vertices,
                edge_types,
                direction,
                max_depth,
                left_vertex_column,
                right_vertex_column,
                single_shortest,
                ..
            } => all_paths::handle_multi_shortest_path(
                &*storage,
                &*space_name,
                &*target_vertices,
                &*edge_types,
                *direction,
                *max_depth,
                &*left_vertex_column,
                &*right_vertex_column,
                *single_shortest,
                base,
                input,
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
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        if base.lifecycle.can_close() {
            match self {
                Self::Expand { .. }
                | Self::ExpandAll { .. }
                | Self::Traverse { .. }
                | Self::TraverseAll { .. }
                | Self::BiExpand { .. }
                | Self::BiTraverse { .. }
                | Self::ShortestPath { .. }
                | Self::BFSShortest { .. }
                | Self::AllPaths { .. }
                | Self::MultiShortestPath { .. }
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
                | Self::ShortestPath { .. }
                | Self::BFSShortest { .. }
                | Self::AllPaths { .. }
                | Self::MultiShortestPath { .. }
                | Self::Subgraph { .. } => {
                    base.lifecycle.mark_closed();
                }
            }
        }
        Ok(())
    }
}
