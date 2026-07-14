use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::storage_ids::VertexId;
use crate::core::{Edge, EdgeDirection, Value};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::context::ValueRowContext;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::executor::traversal::config::TraversalConfig;
use crate::query::executor::traversal::graph_reader::TraversalGraphReader;
use crate::query::executor::traversal::runtime::TraversalRuntime;
use crate::storage::StorageClient;

use super::algorithms::{AllPathsConfig, BidirBfsConfig, bidir_bfs_shortest_path, enumerate_all_paths, path_endpoint_pairs};
use super::visited_set::VisitedSet;

fn row_passes_filter(row: &[Value], col_names: &[String], filter: &Option<Expression>) -> bool {
    let Some(expr) = filter else {
        return true;
    };

    let layout = Arc::new(SlotLayout::from_names(col_names));
    let mut context = ValueRowContext::new(row.to_vec(), layout);
    matches!(
        ExpressionEvaluator::evaluate(expr, &mut context),
        Ok(Value::Bool(true))
    )
}

fn expand_on_chunk(
    chunk: DataChunk,
    reader: &dyn StorageClient,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    filter_expr: &Option<Expression>,
    cancel_token: Option<Arc<AtomicBool>>,
) -> Result<Option<DataChunk>, QueryError> {
    let col_names = chunk.col_names();

    let mut out_rows = Vec::new();
    for row in &chunk.rows {
        let context = ValueRowContext::new(row.clone(), chunk.get_layout());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| row.first().cloned())
            .unwrap_or(Value::Null(crate::core::NullType::Null));

        if let Ok(vid) = VertexId::try_from(&vid_val) {
            let config = TraversalConfig::expand(space_name.to_string(), direction);
            let runtime_reader = TraversalGraphReader::new(reader);
            let mut runtime = TraversalRuntime::new(runtime_reader, config);
            if let Some(token) = cancel_token.clone() {
                runtime.set_cancel_token(token);
            }

            if let Ok(Some(vertex)) = reader.get_vertex(space_name, &vid) {
                runtime.seed_from_vertex(vertex);
            } else {
                continue;
            }

            while let Some(event) = runtime.next_event() {
                let mut out_row = row.clone();
                out_row.push(Value::Vertex(Box::new(event.vertex)));
                out_row.push(Value::String(edge_types.join("/")));
                out_row.push(Value::String(format!("{:?}", direction).to_lowercase()));
                let mut out_col_names = col_names.clone();
                out_col_names.push("_expand_vertex".to_string());
                out_col_names.push("_expand_edge_type".to_string());
                out_col_names.push("_expand_direction".to_string());
                if row_passes_filter(&out_row, &out_col_names, filter_expr) {
                    out_rows.push(out_row);
                }
            }
        }
    }

    if out_rows.is_empty() {
        return Ok(None);
    }

    let mut new_cols: Vec<ColumnInfo> = col_names
        .iter()
        .map(|n| ColumnInfo {
            name: n.clone(),
            data_type: "string".to_string(),
        })
        .collect();
    new_cols.push(ColumnInfo {
        name: "_expand_vertex".to_string(),
        data_type: "vertex".to_string(),
    });
    new_cols.push(ColumnInfo {
        name: "_expand_edge_type".to_string(),
        data_type: "string".to_string(),
    });
    new_cols.push(ColumnInfo {
        name: "_expand_direction".to_string(),
        data_type: "string".to_string(),
    });
    let schema = Arc::new(Schema::new(new_cols));
    Ok(Some(DataChunk::new(out_rows, schema)))
}

fn traverse_on_chunk(
    chunk: DataChunk,
    reader: &dyn StorageClient,
    config: &TraversalConfig,
    visited: &mut VisitedSet,
    cancel_token: Option<Arc<AtomicBool>>,
) -> Result<Option<DataChunk>, QueryError> {
    let col_names = chunk.col_names();
    let edge_type = config.edge_types.first().map(|s| s.as_str()).unwrap_or("");
    let dir_str = match config.direction {
        EdgeDirection::Out => "out",
        EdgeDirection::In => "in",
        EdgeDirection::Both => "both",
    };

    let mut out_rows = Vec::new();
    for row in &chunk.rows {
        let context = ValueRowContext::new(row.clone(), chunk.get_layout());
        let vid_val = context
            .get_variable("vid")
            .or_else(|| row.first().cloned())
            .unwrap_or(Value::Null(crate::core::NullType::Null));
        if let Ok(vid) = VertexId::try_from(&vid_val) {
            let runtime_reader = TraversalGraphReader::new(reader);
            let mut runtime = TraversalRuntime::new(runtime_reader, config.clone());
            if let Some(token) = cancel_token.clone() {
                runtime.set_cancel_token(token);
            }

            if let Ok(Some(vertex)) = reader.get_vertex(&config.space_name, &vid) {
                runtime.seed_from_vertex(vertex);
            } else {
                continue;
            }

            while let Some(event) = runtime.next_event() {
                let nid = event.vertex.vid();
                if !visited.insert(*nid) {
                    continue;
                }

                let mut out_row = row.clone();
                out_row.push(Value::Vertex(Box::new(event.vertex)));
                out_row.push(Value::String(edge_type.to_string()));
                out_row.push(Value::String(dir_str.to_string()));
                out_row.push(Value::BigInt(event.depth as i64));
                out_rows.push(out_row);
            }
        }
    }

    if out_rows.is_empty() {
        return Ok(None);
    }

    let mut new_cols: Vec<ColumnInfo> = col_names
        .iter()
        .map(|n| ColumnInfo {
            name: n.clone(),
            data_type: "string".to_string(),
        })
        .collect();
    new_cols.push(ColumnInfo {
        name: "_traverse_vertex".to_string(),
        data_type: "vertex".to_string(),
    });
    new_cols.push(ColumnInfo {
        name: "_traverse_edge_type".to_string(),
        data_type: "string".to_string(),
    });
    new_cols.push(ColumnInfo {
        name: "_traverse_direction".to_string(),
        data_type: "string".to_string(),
    });
    new_cols.push(ColumnInfo {
        name: "_traverse_depth".to_string(),
        data_type: "bigint".to_string(),
    });
    let schema = Arc::new(Schema::new(new_cols));
    Ok(Some(DataChunk::new(out_rows, schema)))
}

#[derive(Debug)]
pub enum GraphOperator {
    Expand {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
    },
    ExpandAll {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        filter_expr: Option<Expression>,
    },
    Traverse {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        filter_expr: Option<Expression>,
        visited: VisitedSet,
    },
    TraverseAll {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        filter_expr: Option<Expression>,
        visited: VisitedSet,
    },
    BiExpand {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
    },
    BiTraverse {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_depth: u32,
        max_depth: u32,
        visited: VisitedSet,
    },
    ShortestPath {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        start_vertices: Vec<Value>,
        target_vertices: Vec<Value>,
    },
    BFSShortest {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        target_vertex: Option<Expression>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        max_depth: usize,
        allow_cycles: bool,
        allow_loops: bool,
        frontier: Vec<Vec<Value>>,
        visited: VisitedSet,
    },
    AllPaths {
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
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
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
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
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
        steps: u32,
        direction: EdgeDirection,
        edge_types: Vec<String>,
    },
}

impl GraphOperator {
    pub fn bind_runtime(&mut self, runtime: &super::super::runtime::ExecutionRuntime) {
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
        spec: &super::spec::GraphSpec,
        storage: Option<Arc<RwLock<dyn StorageClient>>>,
        space_name: String,
    ) -> Self {
        match spec {
            super::spec::GraphSpec::Expand {
                edge_types,
                direction,
                filter_expr,
            } => Self::Expand {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                filter_expr: filter_expr.clone(),
            },
            super::spec::GraphSpec::ExpandAll {
                edge_types,
                direction,
                filter_expr,
            } => Self::ExpandAll {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                filter_expr: filter_expr.clone(),
            },
            super::spec::GraphSpec::Traverse {
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
            super::spec::GraphSpec::BiExpand {
                edge_types,
                direction,
            } => Self::BiExpand {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
            },
            super::spec::GraphSpec::BiTraverse {
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
            super::spec::GraphSpec::ShortestPath {
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
            super::spec::GraphSpec::BFSShortest {
                target_vertex,
                edge_types,
                direction,
                max_depth,
                allow_cycles,
                allow_loops,
            } => Self::BFSShortest {
                storage: storage.clone(),
                space_name: space_name.clone(),
                target_vertex: target_vertex.clone(),
                edge_types: edge_types.clone(),
                direction: *direction,
                max_depth: *max_depth,
                allow_cycles: *allow_cycles,
                allow_loops: *allow_loops,
                frontier: Vec::new(),
                visited: VisitedSet::new(),
            },
            super::spec::GraphSpec::AllPaths {
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
            super::spec::GraphSpec::MultiShortestPath {
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
        let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());

        match self {
            Self::Expand {
                storage,
                space_name,
                edge_types,
                direction,
                filter_expr,
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("Expand not opened".to_string()));
                }

                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        expand_on_chunk(
                            chunk,
                            &*reader,
                            space_name,
                            edge_types.as_slice(),
                            *direction,
                            filter_expr,
                            cancel_token,
                        )
                    } else {
                        let mut new_cols: Vec<ColumnInfo> = chunk
                            .schema
                            .columns
                            .iter()
                            .map(|c| ColumnInfo {
                                name: c.name.clone(),
                                data_type: c.data_type.clone(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_expand_edge_type".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_expand_direction".to_string(),
                            data_type: "string".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        let mut rows = chunk.rows;
                        for row in rows.iter_mut() {
                            row.push(Value::String(edge_types.join("/")));
                            row.push(Value::String(format!("{:?}", direction).to_lowercase()));
                        }
                        let out_col_names = schema
                            .columns
                            .iter()
                            .map(|c| c.name.clone())
                            .collect::<Vec<_>>();
                        rows.retain(|row| row_passes_filter(row, &out_col_names, filter_expr));
                        Ok(Some(DataChunk::new(rows, schema)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::ExpandAll {
                storage,
                space_name,
                edge_types,
                direction,
                filter_expr,
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("ExpandAll not opened".to_string()));
                }

                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        expand_on_chunk(
                            chunk,
                            &*reader,
                            space_name,
                            edge_types.as_slice(),
                            *direction,
                            filter_expr,
                            cancel_token,
                        )
                    } else {
                        let mut new_cols: Vec<ColumnInfo> = chunk
                            .schema
                            .columns
                            .iter()
                            .map(|c| ColumnInfo {
                                name: c.name.clone(),
                                data_type: c.data_type.clone(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_expand_edge_type".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_expand_direction".to_string(),
                            data_type: "string".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        let mut rows = chunk.rows;
                        for row in rows.iter_mut() {
                            row.push(Value::String(edge_types.join("/")));
                            row.push(Value::String(format!("{:?}", direction).to_lowercase()));
                        }
                        Ok(Some(DataChunk::new(rows, schema)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::Traverse {
                storage,
                space_name,
                edge_types,
                direction,
                min_depth,
                max_depth,
                visited,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("Traverse not opened".to_string()));
                }

                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let tc = TraversalConfig::traverse(
                            space_name.clone(),
                            *direction,
                            *min_depth,
                            *max_depth,
                            edge_types.clone(),
                        );
                        traverse_on_chunk(chunk, &*reader, &tc, visited, cancel_token)
                    } else {
                        let mut new_cols: Vec<ColumnInfo> = chunk
                            .schema
                            .columns
                            .iter()
                            .map(|c| ColumnInfo {
                                name: c.name.clone(),
                                data_type: c.data_type.clone(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_traverse_edge_type".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_traverse_direction".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_traverse_depth".to_string(),
                            data_type: "bigint".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        let mut rows = chunk.rows;
                        for row in rows.iter_mut() {
                            row.push(Value::String(edge_types.join("/")));
                            row.push(Value::String(format!("{:?}", direction).to_lowercase()));
                            row.push(Value::BigInt(1));
                        }
                        Ok(Some(DataChunk::new(rows, schema)))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::TraverseAll { .. } => input.advance(),

            Self::BiExpand {
                storage,
                space_name,
                edge_types,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("BiExpand not opened".to_string()));
                }
                if let Some(chunk) = input.advance()? {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let dir = EdgeDirection::Both;
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            let context =
                                ValueRowContext::new(row.clone(), chunk.get_layout());
                            let vid_val = context
                                .get_variable("vid")
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            if let Ok(vid) = VertexId::try_from(&vid_val) {
                                if let Ok(edges) = reader.get_node_edges(space_name, &vid, dir) {
                                    for e in &edges {
                                        let edge_type_matches = edge_types.is_empty()
                                            || edge_types.contains(&"both".to_string())
                                            || edge_types.contains(&e.edge_type);
                                        if !edge_type_matches {
                                            continue;
                                        }
                                        let neighbor_id =
                                            if e.src() == &vid { *e.dst() } else { *e.src() };
                                        if let Ok(Some(vertex)) =
                                            reader.get_vertex(space_name, &neighbor_id)
                                        {
                                            let mut out_row = row.clone();
                                            out_row.push(Value::Vertex(Box::new(vertex)));
                                            out_row.push(Value::String(e.edge_type.clone()));
                                            out_row.push(Value::String("both".to_string()));
                                            out_rows.push(out_row);
                                        }
                                    }
                                }
                            }
                        }

                        if out_rows.is_empty() {
                            return Ok(None);
                        }
                        let mut new_cols: Vec<ColumnInfo> = col_names
                            .iter()
                            .map(|n| ColumnInfo {
                                name: n.clone(),
                                data_type: "string".to_string(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_expand_vertex".to_string(),
                            data_type: "vertex".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_expand_edge_type".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_expand_direction".to_string(),
                            data_type: "string".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::BiTraverse {
                storage,
                space_name,
                edge_types,
                min_depth,
                max_depth,
                visited,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("BiTraverse not opened".to_string()));
                }
                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let dir = EdgeDirection::Both;
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            let ctx =
                                ValueRowContext::new(row.clone(), chunk.get_layout());
                            let vid_val = ctx
                                .get_variable("vid")
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            if let Ok(vid) = VertexId::try_from(&vid_val) {
                                let mut frontier = vec![(vid, 0u32)];
                                let mut local_visited = VisitedSet::new();
                                local_visited.insert(vid);

                                while let Some((current, depth)) = frontier.pop() {
                                    base.ensure_not_cancelled()?;
                                    if depth >= *max_depth {
                                        continue;
                                    }
                                    if let Ok(edges) =
                                        reader.get_node_edges(space_name, &current, dir)
                                    {
                                        for e in &edges {
                                            let edge_type_matches = edge_types.is_empty()
                                                || edge_types.contains(&"both".to_string())
                                                || edge_types.contains(&e.edge_type);
                                            if !edge_type_matches {
                                                continue;
                                            }
                                            let nid = if e.src() == &current {
                                                *e.dst()
                                            } else {
                                                *e.src()
                                            };
                                            if local_visited.contains(&nid)
                                                || !visited.insert(nid)
                                            {
                                                continue;
                                            }
                                            local_visited.insert(nid);

                                            if depth + 1 >= *min_depth {
                                                if let Ok(Some(vertex)) =
                                                    reader.get_vertex(space_name, &nid)
                                                {
                                                    let mut out_row = row.clone();
                                                    out_row.push(Value::Vertex(Box::new(vertex)));
                                                    out_row
                                                        .push(Value::String(edge_types.join("/")));
                                                    out_row.push(Value::String("both".to_string()));
                                                    out_row.push(Value::BigInt((depth + 1) as i64));
                                                    out_rows.push(out_row);
                                                }
                                            }
                                            frontier.push((nid, depth + 1));
                                        }
                                    }
                                }
                            }
                        }

                        if out_rows.is_empty() {
                            return Ok(None);
                        }
                        let mut new_cols: Vec<ColumnInfo> = col_names
                            .iter()
                            .map(|n| ColumnInfo {
                                name: n.clone(),
                                data_type: "string".to_string(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_traverse_vertex".to_string(),
                            data_type: "vertex".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_traverse_edge_type".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_traverse_direction".to_string(),
                            data_type: "string".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_traverse_depth".to_string(),
                            data_type: "bigint".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::ShortestPath {
                storage,
                space_name,
                target_vertex,
                edge_types,
                max_depth,
                start_vertices,
                target_vertices,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("ShortestPath not opened".to_string()));
                }
                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            for (src_val, dst_val) in path_endpoint_pairs(
                                row,
                                chunk.get_layout(),
                                start_vertices,
                                target_vertices,
                                target_vertex.as_ref(),
                            )? {
                                let (Ok(src_vid), Ok(dst_vid)) =
                                    (VertexId::try_from(&src_val), VertexId::try_from(&dst_val))
                                else {
                                    continue;
                                };
                                let et_ref: Option<&[String]> = if edge_types.is_empty()
                                    || edge_types.contains(&"both".to_string())
                                {
                                    None
                                } else {
                                    Some(edge_types.as_slice())
                                };

                                let cancel_token =
                                    base.runtime.as_ref().map(|rt| rt.cancel_token());
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
                            return Ok(None);
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
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::BFSShortest {
                storage,
                space_name,
                edge_types,
                max_depth,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("BFSShortest not opened".to_string()));
                }
                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            let ctx =
                                ValueRowContext::new(row.clone(), chunk.get_layout());
                            let src_val = ctx
                                .get_variable("vid")
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            let dst_val = ctx
                                .get_variable("dst_vid")
                                .or_else(|| row.get(1).cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));

                            if let (Ok(src_vid), Ok(dst_vid)) =
                                (VertexId::try_from(&src_val), VertexId::try_from(&dst_val))
                            {
                                let et_ref: Option<&[String]> = if edge_types.is_empty()
                                    || edge_types.contains(&"both".to_string())
                                {
                                    None
                                } else {
                                    Some(edge_types.as_slice())
                                };

                                let cancel_token =
                                    base.runtime.as_ref().map(|rt| rt.cancel_token());
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
                            return Ok(None);
                        }
                        let mut new_cols: Vec<ColumnInfo> = col_names
                            .iter()
                            .map(|n| ColumnInfo {
                                name: n.clone(),
                                data_type: "string".to_string(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_bfs_shortest".to_string(),
                            data_type: "path".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }

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
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("AllPaths not opened".to_string()));
                }
                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            for (src_val, dst_val) in path_endpoint_pairs(
                                row,
                                chunk.get_layout(),
                                start_vertices,
                                target_vertices,
                                target_vertex.as_ref(),
                            )? {
                                let (Ok(src_vid), Ok(dst_vid)) =
                                    (VertexId::try_from(&src_val), VertexId::try_from(&dst_val))
                                else {
                                    continue;
                                };
                                let cancel_token =
                                    base.runtime.as_ref().map(|rt| rt.cancel_token());
                                let result_cap =
                                    limit.unwrap_or(usize::MAX).saturating_add(*offset);
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
                                        result_cap,
                                    },
                                    cancel_token.as_deref(),
                                )?;

                                for path in paths
                                    .into_iter()
                                    .skip(*offset)
                                    .take(limit.unwrap_or(usize::MAX))
                                {
                                    base.ensure_not_cancelled()?;
                                    let mut out_row = row.clone();
                                    out_row.push(Value::Path(Box::new(path)));
                                    if row_passes_filter(&out_row, &col_names, filter) {
                                        out_rows.push(out_row);
                                    }
                                }
                            }
                        }

                        if out_rows.is_empty() {
                            return Ok(None);
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
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::MultiShortestPath {
                storage,
                space_name,
                target_vertices,
                edge_types,
                max_depth,
                left_vertex_column,
                right_vertex_column,
                single_shortest,
                ..
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution(
                        "MultiShortestPath not opened".to_string(),
                    ));
                }
                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            let ctx =
                                ValueRowContext::new(row.clone(), chunk.get_layout());
                            let src_val = (!left_vertex_column.is_empty())
                                .then(|| ctx.get_variable(left_vertex_column))
                                .flatten()
                                .or_else(|| ctx.get_variable("vid"))
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));
                            let mut dst_values = Vec::new();
                            if let Some(value) = (!right_vertex_column.is_empty())
                                .then(|| ctx.get_variable(right_vertex_column))
                                .flatten()
                                .or_else(|| ctx.get_variable("dst_vid"))
                                .or_else(|| row.get(1).cloned())
                            {
                                dst_values.push(value);
                            }
                            for expression in target_vertices.iter() {
                                let mut expression_context = ValueRowContext::new(
                                    row.clone(),
                                    chunk.get_layout(),
                                );
                                dst_values.push(
                                    ExpressionEvaluator::evaluate(
                                        expression,
                                        &mut expression_context,
                                    )
                                    .map_err(|error| {
                                        QueryError::execution(format!(
                                            "MultiShortestPath target evaluation failed: {error}"
                                        ))
                                    })?,
                                );
                            }
                            if let Ok(src_vid) = VertexId::try_from(&src_val) {
                                let et_ref: Option<&[String]> = if edge_types.is_empty()
                                    || edge_types.contains(&"both".to_string())
                                {
                                    None
                                } else {
                                    Some(edge_types.as_slice())
                                };

                                for dst_value in dst_values {
                                    let Ok(dst_vid) = VertexId::try_from(&dst_value) else {
                                        continue;
                                    };
                                    base.ensure_not_cancelled()?;
                                    let cancel_token =
                                        base.runtime.as_ref().map(|rt| rt.cancel_token());
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
                        }

                        if out_rows.is_empty() {
                            return Ok(None);
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
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }

            Self::Subgraph {
                storage,
                space_name,
                steps,
                direction,
                edge_types,
            } => {
                if !base.lifecycle.is_opened() {
                    return Err(QueryError::execution("Subgraph not opened".to_string()));
                }
                let chunk = input.advance()?;
                if let Some(chunk) = chunk {
                    if let Some(storage_lock) = storage {
                        let reader = storage_lock.read();
                        let col_names = chunk.col_names();

                        let mut out_rows = Vec::new();
                        for row in &chunk.rows {
                            base.ensure_not_cancelled()?;
                            let ctx =
                                ValueRowContext::new(row.clone(), chunk.get_layout());
                            let vid_val = ctx
                                .get_variable("vid")
                                .or_else(|| row.first().cloned())
                                .unwrap_or(Value::Null(crate::core::NullType::Null));

                            if let Ok(seed_vid) = VertexId::try_from(&vid_val) {
                                let mut visited: HashSet<VertexId> = HashSet::new();
                                let mut history_edges: Vec<(Edge, u32)> = Vec::new();
                                let mut frontier = vec![(seed_vid, 0u32)];
                                visited.insert(seed_vid);

                                while let Some((current, current_step)) = frontier.pop() {
                                    base.ensure_not_cancelled()?;
                                    if current_step >= *steps {
                                        continue;
                                    }
                                    if let Ok(edges) =
                                        reader.get_node_edges(space_name, &current, *direction)
                                    {
                                        let et_set: HashSet<String> =
                                            edge_types.iter().cloned().collect();
                                        for e in &edges {
                                            if !edge_types.is_empty()
                                                && !et_set.contains(&e.edge_type)
                                            {
                                                continue;
                                            }
                                            let neighbor_id = match direction {
                                                EdgeDirection::Out => *e.dst(),
                                                EdgeDirection::In => *e.src(),
                                                EdgeDirection::Both => {
                                                    if e.src() == &current {
                                                        *e.dst()
                                                    } else {
                                                        *e.src()
                                                    }
                                                }
                                            };
                                            history_edges.push((e.clone(), current_step + 1));
                                            if visited.insert(neighbor_id)
                                                && current_step + 1 < *steps
                                            {
                                                frontier.push((neighbor_id, current_step + 1));
                                            }
                                        }
                                    }
                                }

                                for (edge, _step) in &history_edges {
                                    let mut out_row = row.clone();
                                    let src_vertex = reader
                                        .get_vertex(space_name, &edge.src)
                                        .ok()
                                        .flatten()
                                        .unwrap_or_else(|| {
                                            crate::core::vertex_edge_path::Vertex::with_vid(
                                                edge.src,
                                            )
                                        });
                                    let dst_vertex = reader
                                        .get_vertex(space_name, &edge.dst)
                                        .ok()
                                        .flatten()
                                        .unwrap_or_else(|| {
                                            crate::core::vertex_edge_path::Vertex::with_vid(
                                                edge.dst,
                                            )
                                        });
                                    out_row.push(Value::Vertex(Box::new(src_vertex)));
                                    out_row.push(Value::Vertex(Box::new(dst_vertex)));
                                    out_row.push(Value::String(edge.edge_type.clone()));
                                    out_rows.push(out_row);
                                }
                            }
                        }

                        if out_rows.is_empty() {
                            return Ok(None);
                        }
                        let mut new_cols: Vec<ColumnInfo> = col_names
                            .iter()
                            .map(|n| ColumnInfo {
                                name: n.clone(),
                                data_type: "string".to_string(),
                            })
                            .collect();
                        new_cols.push(ColumnInfo {
                            name: "_subgraph_src".to_string(),
                            data_type: "vertex".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_subgraph_dst".to_string(),
                            data_type: "vertex".to_string(),
                        });
                        new_cols.push(ColumnInfo {
                            name: "_subgraph_edge_type".to_string(),
                            data_type: "string".to_string(),
                        });
                        let schema = Arc::new(Schema::new(new_cols));
                        Ok(Some(DataChunk::new(out_rows, schema)))
                    } else {
                        Ok(Some(chunk))
                    }
                } else {
                    Ok(None)
                }
            }
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
                    input.stop()?;
                    base.lifecycle.mark_stopped();
                }
            }
        }
        Ok(())
    }

    pub fn close(
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
                    input.close()?;
                    base.lifecycle.mark_closed();
                }
            }
        }
        Ok(())
    }
}
