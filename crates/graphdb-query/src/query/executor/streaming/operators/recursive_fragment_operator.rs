use std::sync::Arc;

use parking_lot::RwLock;

use super::algorithms::{
    bidir_bfs_shortest_path, enumerate_all_paths, path_endpoint_pairs, AllPathsConfig,
    BidirBfsConfig,
};
use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::storage::QueryStorage;

use super::spec::RecursiveFragmentSpec;

#[derive(Debug)]
pub enum RecursiveFragmentOperator {
    ShortestPath {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
    },
    MultiShortestPath {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        left_vertex_column: String,
        right_vertex_column: String,
        single_shortest: bool,
    },
    BFSShortest {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        allow_loops: bool,
    },
    AllPaths {
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: usize,
        max_depth: usize,
        acyclic: bool,
        limit: Option<usize>,
        offset: usize,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
    },
}

impl RecursiveFragmentOperator {
    pub fn from_spec(
        spec: &RecursiveFragmentSpec,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
    ) -> Self {
        match spec {
            RecursiveFragmentSpec::ShortestPath {
                edge_types,
                direction,
                max_depth,
                start_vertices,
                target_vertices,
            } => Self::ShortestPath {
                storage,
                space_name,
                edge_types: edge_types.clone(),
                direction: *direction,
                max_depth: *max_depth,
                start_vertices: start_vertices.clone(),
                target_vertices: target_vertices.clone(),
            },
            RecursiveFragmentSpec::MultiShortestPath {
                edge_types,
                direction,
                max_depth,
                left_vertex_column,
                right_vertex_column,
                single_shortest,
            } => Self::MultiShortestPath {
                storage,
                space_name,
                edge_types: edge_types.clone(),
                direction: *direction,
                max_depth: *max_depth,
                left_vertex_column: left_vertex_column.clone(),
                right_vertex_column: right_vertex_column.clone(),
                single_shortest: *single_shortest,
            },
            RecursiveFragmentSpec::BFSShortest {
                edge_types,
                direction,
                max_depth,
                allow_loops,
            } => Self::BFSShortest {
                storage,
                space_name,
                edge_types: edge_types.clone(),
                direction: *direction,
                max_depth: *max_depth,
                allow_loops: *allow_loops,
            },
            RecursiveFragmentSpec::AllPaths {
                edge_types,
                direction,
                min_depth,
                max_depth,
                acyclic,
                limit,
                offset,
                start_vertices,
                target_vertices,
            } => Self::AllPaths {
                storage,
                space_name,
                edge_types: edge_types.clone(),
                direction: *direction,
                min_depth: *min_depth,
                max_depth: *max_depth,
                acyclic: *acyclic,
                limit: *limit,
                offset: *offset,
                start_vertices: start_vertices.clone(),
                target_vertices: target_vertices.clone(),
            },
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        input.open()?;
        base.lifecycle.mark_opened();
        Ok(())
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        input: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        if !base.lifecycle.is_opened() {
            return Err(QueryError::execution("RecursiveFragment not opened"));
        }

        match self {
            Self::ShortestPath {
                storage,
                space_name,
                edge_types,
                direction,
                max_depth,
                start_vertices,
                target_vertices,
            } => loop {
                let Some(chunk) = input.advance()? else {
                    return Ok(None);
                };
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let col_names = chunk.col_names();
                    let layout = chunk.get_layout();
                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        base.ensure_not_cancelled()?;
                        let pairs = path_endpoint_pairs(
                            row,
                            layout.clone(),
                            start_vertices,
                            target_vertices,
                            None,
                        )?;
                        let et_ref: Option<&[String]> = if edge_types.is_empty() {
                            None
                        } else {
                            Some(edge_types.as_slice())
                        };
                        for (src_val, dst_val) in pairs {
                            let Ok(src_vid) = VertexId::try_from(&src_val) else {
                                continue;
                            };
                            let Ok(dst_vid) = VertexId::try_from(&dst_val) else {
                                continue;
                            };
                            base.ensure_not_cancelled()?;
                            let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
                            let paths = bidir_bfs_shortest_path(
                                &*reader,
                                &src_vid,
                                &dst_vid,
                                BidirBfsConfig {
                                    space_name,
                                    edge_type_filter: et_ref,
                                    max_depth: *max_depth,
                                    single_shortest: true,
                                    limit: 1,
                                    direction: *direction,
                                },
                                cancel_token.as_deref(),
                            )?;
                            for path in &paths {
                                base.ensure_not_cancelled()?;
                                let mut out_row = row.clone();
                                out_row.push(Value::Path(Box::new(path.clone())));
                                out_rows.push(out_row);
                            }
                        }
                    }
                    if out_rows.is_empty() {
                        continue;
                    }
                    let mut new_cols: Vec<ColumnInfo> = col_names
                        .iter()
                        .map(|n| ColumnInfo {
                            name: n.clone(),
                            data_type: "string".to_string(),
                        })
                        .collect();
                    new_cols.push(ColumnInfo {
                        name: "_shortest_path".to_string(),
                        data_type: "path".to_string(),
                    });
                    let _schema = Arc::new(Schema::new(new_cols));
                    return Ok(Some(DataChunk::new_with_layout(
                        out_rows,
                        Arc::clone(&base.output_layout),
                    )));
                } else {
                    return Ok(Some(chunk));
                }
            },

            Self::MultiShortestPath {
                storage,
                space_name,
                edge_types,
                direction,
                max_depth,
                left_vertex_column,
                right_vertex_column,
                single_shortest,
                ..
            } => loop {
                let Some(chunk) = input.advance()? else {
                    return Ok(None);
                };
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let col_names = chunk.col_names();
                    let layout = chunk.get_layout();
                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        base.ensure_not_cancelled()?;
                        let ctx = ValueRowContext::new(row.clone(), layout.clone());
                        let left_val = ctx
                            .get_variable(left_vertex_column)
                            .or_else(|| ctx.get_variable("vid"))
                            .or_else(|| row.first().cloned())
                            .unwrap_or(Value::Null(crate::core::NullType::Null));
                        let right_val = ctx
                            .get_variable(right_vertex_column)
                            .or_else(|| ctx.get_variable("dst_vid"))
                            .or_else(|| row.get(1).cloned())
                            .unwrap_or(Value::Null(crate::core::NullType::Null));
                        let Ok(src_vid) = VertexId::try_from(&left_val) else {
                            continue;
                        };
                        let Ok(dst_vid) = VertexId::try_from(&right_val) else {
                            continue;
                        };
                        let et_ref: Option<&[String]> = if edge_types.is_empty() {
                            None
                        } else {
                            Some(edge_types.as_slice())
                        };
                        base.ensure_not_cancelled()?;
                        let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
                        let paths = bidir_bfs_shortest_path(
                            &*reader,
                            &src_vid,
                            &dst_vid,
                            BidirBfsConfig {
                                space_name,
                                edge_type_filter: et_ref,
                                max_depth: *max_depth,
                                single_shortest: *single_shortest,
                                limit: if *single_shortest { 1 } else { 10 },
                                direction: *direction,
                            },
                            cancel_token.as_deref(),
                        )?;
                        for path in &paths {
                            base.ensure_not_cancelled()?;
                            let mut out_row = row.clone();
                            out_row.push(Value::Path(Box::new(path.clone())));
                            out_rows.push(out_row);
                        }
                    }
                    if out_rows.is_empty() {
                        continue;
                    }
                    let mut new_cols: Vec<ColumnInfo> = col_names
                        .iter()
                        .map(|n| ColumnInfo {
                            name: n.clone(),
                            data_type: "string".to_string(),
                        })
                        .collect();
                    new_cols.push(ColumnInfo {
                        name: "_multi_shortest_path".to_string(),
                        data_type: "path".to_string(),
                    });
                    let _schema = Arc::new(Schema::new(new_cols));
                    return Ok(Some(DataChunk::new_with_layout(
                        out_rows,
                        Arc::clone(&base.output_layout),
                    )));
                } else {
                    return Ok(Some(chunk));
                }
            },

            Self::BFSShortest {
                storage,
                space_name,
                edge_types,
                direction,
                max_depth,
                allow_loops,
            } => loop {
                let Some(chunk) = input.advance()? else {
                    return Ok(None);
                };
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let col_names = chunk.col_names();
                    let layout = chunk.get_layout();
                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        base.ensure_not_cancelled()?;
                        let ctx = ValueRowContext::new(row.clone(), layout.clone());
                        let vid_val = ctx
                            .get_variable("vid")
                            .or_else(|| row.first().cloned())
                            .unwrap_or(Value::Null(crate::core::NullType::Null));
                        let Ok(start_vid) = VertexId::try_from(&vid_val) else {
                            continue;
                        };
                        let end_val = ctx
                            .get_variable("dst_vid")
                            .or_else(|| ctx.get_variable("target"))
                            .or_else(|| row.get(1).cloned())
                            .unwrap_or(Value::Null(crate::core::NullType::Null));
                        let Ok(end_vid) = VertexId::try_from(&end_val) else {
                            continue;
                        };
                        let et_ref: Option<&[String]> = if edge_types.is_empty() {
                            None
                        } else {
                            Some(edge_types.as_slice())
                        };
                        let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
                        let paths = bidir_bfs_shortest_path(
                            &*reader,
                            &start_vid,
                            &end_vid,
                            BidirBfsConfig {
                                space_name,
                                edge_type_filter: et_ref,
                                max_depth: *max_depth,
                                single_shortest: !*allow_loops,
                                limit: if *allow_loops { 1000 } else { 1 },
                                direction: *direction,
                            },
                            cancel_token.as_deref(),
                        )?;
                        for path in &paths {
                            base.ensure_not_cancelled()?;
                            let mut out_row = row.clone();
                            out_row.push(Value::Path(Box::new(path.clone())));
                            out_rows.push(out_row);
                        }
                    }
                    if out_rows.is_empty() {
                        continue;
                    }
                    let mut new_cols: Vec<ColumnInfo> = col_names
                        .iter()
                        .map(|n| ColumnInfo {
                            name: n.clone(),
                            data_type: "string".to_string(),
                        })
                        .collect();
                    new_cols.push(ColumnInfo {
                        name: "_bfs_path".to_string(),
                        data_type: "path".to_string(),
                    });
                    let _schema = Arc::new(Schema::new(new_cols));
                    return Ok(Some(DataChunk::new_with_layout(
                        out_rows,
                        Arc::clone(&base.output_layout),
                    )));
                } else {
                    return Ok(Some(chunk));
                }
            },

            Self::AllPaths {
                storage,
                space_name,
                edge_types,
                direction,
                min_depth,
                max_depth,
                acyclic,
                limit,
                offset,
                start_vertices,
                target_vertices,
            } => loop {
                let Some(chunk) = input.advance()? else {
                    return Ok(None);
                };
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let col_names = chunk.col_names();
                    let layout = chunk.get_layout();
                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        base.ensure_not_cancelled()?;
                        let pairs = path_endpoint_pairs(
                            row,
                            layout.clone(),
                            start_vertices,
                            target_vertices,
                            None,
                        )?;
                        for (src_val, dst_val) in pairs {
                            let Ok(src_vid) = VertexId::try_from(&src_val) else {
                                continue;
                            };
                            let Ok(dst_vid) = VertexId::try_from(&dst_val) else {
                                continue;
                            };
                            let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
                            let paths = enumerate_all_paths(
                                &*reader,
                                &src_vid,
                                &dst_vid,
                                AllPathsConfig {
                                    space_name,
                                    edge_types,
                                    direction: *direction,
                                    min_depth: *min_depth,
                                    max_depth: *max_depth,
                                    acyclic: *acyclic,
                                    result_cap: limit.unwrap_or(usize::MAX),
                                },
                                cancel_token.as_deref(),
                            )?;
                            for path in paths.iter().skip(*offset) {
                                base.ensure_not_cancelled()?;
                                let mut out_row = row.clone();
                                out_row.push(Value::Path(Box::new(path.clone())));
                                out_rows.push(out_row);
                            }
                        }
                    }
                    if out_rows.is_empty() {
                        continue;
                    }
                    let mut new_cols: Vec<ColumnInfo> = col_names
                        .iter()
                        .map(|n| ColumnInfo {
                            name: n.clone(),
                            data_type: "string".to_string(),
                        })
                        .collect();
                    new_cols.push(ColumnInfo {
                        name: "_all_paths".to_string(),
                        data_type: "path".to_string(),
                    });
                    let _schema = Arc::new(Schema::new(new_cols));
                    return Ok(Some(DataChunk::new_with_layout(
                        out_rows,
                        Arc::clone(&base.output_layout),
                    )));
                } else {
                    return Ok(Some(chunk));
                }
            },
        }
    }

    pub fn stop(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        if base.lifecycle.can_close() {
            base.lifecycle.mark_stopped();
        }
        Ok(())
    }

    pub fn bind_runtime(&mut self, runtime: &super::super::runtime::ExecutionRuntime) {
        let storage = runtime.storage.clone();
        let space_name = runtime.query_id().space_name.clone().unwrap_or_default();
        match self {
            Self::ShortestPath {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | Self::MultiShortestPath {
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
            } => {
                *target_storage = storage;
                *target_space = space_name;
            }
        }
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        _input: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        base.lifecycle.mark_closed();
        Ok(())
    }
}
