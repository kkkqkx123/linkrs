use std::sync::Arc;

use parking_lot::RwLock;

use super::algorithms::{
    bidir_bfs_shortest_path, enumerate_all_paths, path_endpoint_pairs, AllPathsConfig,
    BidirBfsConfig,
};
use crate::executor::expression::evaluator::traits::ExpressionContext;
use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::context::ValueRowContext;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use crate::storage::QueryStorage;
use graphdb_core::error::QueryError;
use graphdb_core::types::storage_ids::VertexId;
use graphdb_core::{EdgeDirection, Value};

use super::spec::RecursiveFragmentSpec;

#[derive(Debug)]
pub enum RecursiveFragmentOperatorKind {
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
    Fixpoint {
        /// Mangled CTE tag identifying the working table on the runtime.
        cte_name: String,
        /// Anchor sub-plan (runs once, seeds the working table).
        anchor: Arc<crate::executor::streaming::plan::types::PhysicalPlan>,
        /// Step sub-plan (re-runs per iteration over the delta).
        step: Option<Arc<crate::executor::streaming::plan::types::PhysicalPlan>>,
        /// Fixpoint iteration cap (errors when exceeded).
        max_iterations: u64,
        /// Output column names (single column in V1).
        col_names: Vec<String>,
        /// Lazily materialized child executors, reused across iterations.
        state: FixpointState,
    },
}

/// Mutable fixpoint execution state, owned by the operator.
///
/// Sub-plan executors are built and dropped inside a single `run_fixpoint`
/// call; only the converged result is retained here.
#[derive(Debug, Default)]
pub struct FixpointState {
    /// Final converged rows once computed; served chunk by chunk.
    output: Option<Vec<Vec<Value>>>,
    output_pos: usize,
}

/// Recursive fragment operator.
///
/// Wraps [`RecursiveFragmentOperatorKind`] with the runtime context injected
/// at `open()`. Lifecycle state is owned exclusively by the executor;
/// operators never write it.
#[derive(Debug)]
pub struct RecursiveFragmentOperator {
    pub kind: RecursiveFragmentOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl RecursiveFragmentOperator {
    pub fn from_spec(
        spec: &RecursiveFragmentSpec,
        storage: Option<Arc<RwLock<dyn QueryStorage>>>,
        space_name: String,
        output_layout: Arc<SlotLayout>,
    ) -> Self {
        let kind = match spec {
            RecursiveFragmentSpec::ShortestPath {
                edge_types,
                direction,
                max_depth,
                start_vertices,
                target_vertices,
            } => RecursiveFragmentOperatorKind::ShortestPath {
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
            } => RecursiveFragmentOperatorKind::MultiShortestPath {
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
            } => RecursiveFragmentOperatorKind::BFSShortest {
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
            } => RecursiveFragmentOperatorKind::AllPaths {
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
            RecursiveFragmentSpec::Fixpoint {
                cte_name,
                anchor,
                step,
                max_iterations,
                col_names,
            } => RecursiveFragmentOperatorKind::Fixpoint {
                cte_name: cte_name.clone(),
                anchor: anchor.clone(),
                step: step.clone(),
                max_iterations: *max_iterations,
                col_names: col_names.clone(),
                state: FixpointState::default(),
            },
        };
        Self::new(kind, output_layout)
    }

    pub fn new(kind: RecursiveFragmentOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
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
        input.open()?;
        Ok(())
    }

    pub fn next(&mut self, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
        match &mut self.kind {
            RecursiveFragmentOperatorKind::ShortestPath {
                storage,
                space_name,
                edge_types,
                direction,
                max_depth,
                start_vertices,
                target_vertices,
            } => loop {
                let Some(mut chunk) = input.advance()? else {
                    return Ok(None);
                };
                chunk.normalize_for_opaque("RecursiveFragment");
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let col_names = chunk.col_names();
                    let layout = chunk.get_layout();
                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        if let Some(rt) = self.runtime.as_ref() {
                            rt.ensure_not_cancelled()?;
                        }
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
                            if let Some(rt) = self.runtime.as_ref() {
                                rt.ensure_not_cancelled()?;
                            }
                            let cancel_token = self.runtime.as_ref().map(|rt| rt.cancel_token());
                            let paths = bidir_bfs_shortest_path(
                                &*reader,
                                &src_vid,
                                &dst_vid,
                                BidirBfsConfig {
                                    space_name,
                                    edge_type_filter: et_ref,
                                    max_depth: *max_depth,
                                    single_shortest: false,
                                    limit: 1000,
                                    direction: *direction,
                                },
                                cancel_token.as_ref(),
                            )?;
                            for path in &paths {
                                if let Some(rt) = self.runtime.as_ref() {
                                    rt.ensure_not_cancelled()?;
                                }
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
                        name: "path".to_string(),
                        data_type: "path".to_string(),
                    });
                    let _schema = Arc::new(Schema::new(new_cols));
                    return Ok(Some(DataChunk::new_with_layout(
                        out_rows,
                        Arc::clone(&self.output_layout),
                    )));
                } else {
                    return Ok(Some(chunk));
                }
            },

            RecursiveFragmentOperatorKind::MultiShortestPath {
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
                let Some(mut chunk) = input.advance()? else {
                    return Ok(None);
                };
                chunk.normalize_for_opaque("RecursiveFragment");
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let col_names = chunk.col_names();
                    let layout = chunk.get_layout();
                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        if let Some(rt) = self.runtime.as_ref() {
                            rt.ensure_not_cancelled()?;
                        }
                        let ctx = ValueRowContext::new(row.clone(), layout.clone());
                        let left_val = ctx
                            .get_variable(left_vertex_column)
                            .or_else(|| ctx.get_variable("vid"))
                            .or_else(|| row.first().cloned())
                            .unwrap_or(Value::Null(graphdb_core::NullType::Null));
                        let right_val = ctx
                            .get_variable(right_vertex_column)
                            .or_else(|| ctx.get_variable("dst_vid"))
                            .or_else(|| row.get(1).cloned())
                            .unwrap_or(Value::Null(graphdb_core::NullType::Null));
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
                        if let Some(rt) = self.runtime.as_ref() {
                            rt.ensure_not_cancelled()?;
                        }
                        let cancel_token = self.runtime.as_ref().map(|rt| rt.cancel_token());
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
                            cancel_token.as_ref(),
                        )?;
                        for path in &paths {
                            if let Some(rt) = self.runtime.as_ref() {
                                rt.ensure_not_cancelled()?;
                            }
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
                        Arc::clone(&self.output_layout),
                    )));
                } else {
                    return Ok(Some(chunk));
                }
            },

            RecursiveFragmentOperatorKind::BFSShortest {
                storage,
                space_name,
                edge_types,
                direction,
                max_depth,
                allow_loops,
            } => loop {
                let Some(mut chunk) = input.advance()? else {
                    return Ok(None);
                };
                chunk.normalize_for_opaque("RecursiveFragment");
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let col_names = chunk.col_names();
                    let layout = chunk.get_layout();
                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        if let Some(rt) = self.runtime.as_ref() {
                            rt.ensure_not_cancelled()?;
                        }
                        let ctx = ValueRowContext::new(row.clone(), layout.clone());
                        let vid_val = ctx
                            .get_variable("vid")
                            .or_else(|| row.first().cloned())
                            .unwrap_or(Value::Null(graphdb_core::NullType::Null));
                        let Ok(start_vid) = VertexId::try_from(&vid_val) else {
                            continue;
                        };
                        let end_val = ctx
                            .get_variable("dst_vid")
                            .or_else(|| ctx.get_variable("target"))
                            .or_else(|| row.get(1).cloned())
                            .unwrap_or(Value::Null(graphdb_core::NullType::Null));
                        let Ok(end_vid) = VertexId::try_from(&end_val) else {
                            continue;
                        };
                        let et_ref: Option<&[String]> = if edge_types.is_empty() {
                            None
                        } else {
                            Some(edge_types.as_slice())
                        };
                        let cancel_token = self.runtime.as_ref().map(|rt| rt.cancel_token());
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
                            cancel_token.as_ref(),
                        )?;
                        for path in &paths {
                            if let Some(rt) = self.runtime.as_ref() {
                                rt.ensure_not_cancelled()?;
                            }
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
                        Arc::clone(&self.output_layout),
                    )));
                } else {
                    return Ok(Some(chunk));
                }
            },

            RecursiveFragmentOperatorKind::AllPaths {
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
                let Some(mut chunk) = input.advance()? else {
                    return Ok(None);
                };
                chunk.normalize_for_opaque("RecursiveFragment");
                if let Some(storage_lock) = storage {
                    let reader = storage_lock.read();
                    let col_names = chunk.col_names();
                    let layout = chunk.get_layout();
                    let mut out_rows = Vec::new();
                    for row in &chunk.rows {
                        if let Some(rt) = self.runtime.as_ref() {
                            rt.ensure_not_cancelled()?;
                        }
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
                            let cancel_token = self.runtime.as_ref().map(|rt| rt.cancel_token());
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
                                cancel_token.as_ref(),
                            )?;
                            for path in paths.iter().skip(*offset) {
                                if let Some(rt) = self.runtime.as_ref() {
                                    rt.ensure_not_cancelled()?;
                                }
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
                        name: "path".to_string(),
                        data_type: "path".to_string(),
                    });
                    let _schema = Arc::new(Schema::new(new_cols));
                    return Ok(Some(DataChunk::new_with_layout(
                        out_rows,
                        Arc::clone(&self.output_layout),
                    )));
                } else {
                    return Ok(Some(chunk));
                }
            },
            RecursiveFragmentOperatorKind::Fixpoint { .. } => self.next_fixpoint(),
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn bind_runtime(&mut self, runtime: &Arc<ExecutionRuntime>) {
        let storage = runtime.storage.clone();
        let space_name = runtime.query_id().space_name.clone().unwrap_or_default();
        self.runtime = Some(Arc::clone(runtime));
        match &mut self.kind {
            RecursiveFragmentOperatorKind::ShortestPath {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | RecursiveFragmentOperatorKind::MultiShortestPath {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | RecursiveFragmentOperatorKind::BFSShortest {
                storage: target_storage,
                space_name: target_space,
                ..
            }
            | RecursiveFragmentOperatorKind::AllPaths {
                storage: target_storage,
                space_name: target_space,
                ..
            } => {
                *target_storage = storage;
                *target_space = space_name;
            }
            RecursiveFragmentOperatorKind::Fixpoint { .. } => {
                // Child sub-plans inherit storage from the shared runtime at
                // materialization time; nothing to rebind here.
            }
        }
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    /// Serve the converged fixpoint rows chunk by chunk, computing them on
    /// first call. The (ignored) fragment input is never advanced: fixpoint
    /// fragments are always built over a `Start` source.
    fn next_fixpoint(&mut self) -> Result<Option<DataChunk>, QueryError> {
        let (cte_name, anchor, step, max_iterations, state, output_layout, chunk_size) =
            match &mut self.kind {
                RecursiveFragmentOperatorKind::Fixpoint {
                    cte_name,
                    anchor,
                    step,
                    max_iterations,
                    state,
                    ..
                } => (
                    cte_name.clone(),
                    anchor.clone(),
                    step.clone(),
                    *max_iterations,
                    state,
                    Arc::clone(&self.output_layout),
                    self.config.chunk_size,
                ),
                _ => {
                    return Err(QueryError::execution(
                        "next_fixpoint called for a non-fixpoint fragment".to_string(),
                    ));
                }
            };
        let runtime = self.runtime.clone().ok_or_else(|| {
            QueryError::execution("Fixpoint execution requires a runtime".to_string())
        })?;

        if state.output.is_none() {
            let rows =
                Self::run_fixpoint(&runtime, &cte_name, &anchor, step.as_ref(), max_iterations)?;
            state.output = Some(rows);
        }
        let output = state.output.as_ref().expect("computed above");
        if state.output_pos >= output.len() {
            return Ok(None);
        }
        let end = (state.output_pos + chunk_size).min(output.len());
        let rows = output[state.output_pos..end].to_vec();
        state.output_pos = end;
        Ok(Some(DataChunk::new_with_layout(rows, output_layout)))
    }

    /// Run anchor once, then iterate the step over successive deltas until
    /// no new rows appear or `max_iterations` is exceeded.
    fn run_fixpoint(
        runtime: &Arc<ExecutionRuntime>,
        cte_name: &str,
        anchor: &Arc<crate::executor::streaming::plan::types::PhysicalPlan>,
        step: Option<&Arc<crate::executor::streaming::plan::types::PhysicalPlan>>,
        max_iterations: u64,
    ) -> Result<Vec<Vec<Value>>, QueryError> {
        let bindings = Arc::new(Self::child_bindings(runtime));

        let mut anchor_exec = Self::materialize_child(runtime, &bindings, anchor)?;
        let anchor_rows = Self::collect_all(&mut anchor_exec)?;

        let mut seen_keys = std::collections::HashSet::new();
        let mut seen_rows: Vec<Vec<Value>> = Vec::new();
        for row in anchor_rows {
            let key = postcard::to_allocvec(&row).map_err(|e| {
                QueryError::execution(format!("Fixpoint row serialization failed: {e}"))
            })?;
            if seen_keys.insert(key) {
                seen_rows.push(row);
            }
        }

        let Some(step_plan) = step else {
            // Non-recursive CTE: the anchor output is the whole result.
            return Ok(seen_rows);
        };

        let mut step_exec = Self::materialize_child(runtime, &bindings, step_plan)?;
        let mut delta: Vec<Vec<Value>> = seen_rows.clone();
        for _ in 0..max_iterations {
            runtime.ensure_not_cancelled()?;
            if delta.is_empty() {
                runtime.clear_cte_table(cte_name);
                return Ok(seen_rows);
            }
            runtime.set_cte_table(cte_name, delta);
            step_exec.reset()?;
            let step_rows = Self::collect_all(&mut step_exec)?;
            let mut next_delta = Vec::new();
            for row in step_rows {
                let key = postcard::to_allocvec(&row).map_err(|e| {
                    QueryError::execution(format!("Fixpoint row serialization failed: {e}"))
                })?;
                if seen_keys.insert(key) {
                    next_delta.push(row.clone());
                    seen_rows.push(row);
                }
            }
            delta = next_delta;
        }
        runtime.clear_cte_table(cte_name);
        if delta.is_empty() {
            Ok(seen_rows)
        } else {
            Err(QueryError::execution(format!(
                "Recursive CTE did not converge within {max_iterations} iterations"
            )))
        }
    }

    /// Materialize a child sub-plan, inheriting the parent runtime (storage,
    /// cancellation, parameters, CTE tables) like expression subqueries do.
    fn materialize_child(
        runtime: &Arc<ExecutionRuntime>,
        bindings: &Arc<crate::executor::streaming::instance::QueryBindings>,
        plan: &Arc<crate::executor::streaming::plan::types::PhysicalPlan>,
    ) -> Result<StreamingExecutor, QueryError> {
        use crate::executor::streaming::plan::materializer::PhysicalPlanMaterializer;

        let (mut exec, _) = PhysicalPlanMaterializer::materialize(plan, bindings)?;
        exec.set_chunk_size(bindings.chunk_size);
        exec.set_runtime(Some(runtime.clone()));
        exec.open()?;
        Ok(exec)
    }

    /// Run an executor to completion, collecting every output row.
    fn collect_all(exec: &mut StreamingExecutor) -> Result<Vec<Vec<Value>>, QueryError> {
        let mut rows = Vec::new();
        exec.reset()?;
        while let Some(mut chunk) = exec.advance()? {
            chunk.normalize_for_opaque("RecursiveFixpoint");
            rows.extend(chunk.rows.drain(..));
        }
        Ok(rows)
    }

    /// Child bindings for fixpoint sub-plans, derived from the parent
    /// runtime. Serial execution (`max_workers = 1`): fixpoint fragments are
    /// never partitioned.
    fn child_bindings(
        runtime: &Arc<ExecutionRuntime>,
    ) -> crate::executor::streaming::instance::QueryBindings {
        use crate::executor::streaming::instance::QueryBindings;
        use crate::executor::streaming::transaction_scope::TransactionScope;

        let identity = runtime.query_id();
        QueryBindings {
            parameters: runtime.parameter_values.clone().unwrap_or_default(),
            session_variables: runtime.session_variable_values.clone().unwrap_or_default(),
            parameter_frame: None,
            space_name: identity.space_name.clone(),
            storage: runtime.storage.clone(),
            bound_snapshot: None,
            memory_budget: runtime.memory_budget.clone(),
            max_workers: 1,
            chunk_size: crate::executor::base::ExecutionContext::DEFAULT_CHUNK_SIZE,
            max_buffered_chunks:
                crate::executor::base::ExecutionContext::DEFAULT_MAX_BUFFERED_CHUNKS,
            query_id: identity.query_id,
            cancel_token: Some(runtime.cancel_token()),
            query_text: None,
            session_id: identity.session_id.clone(),
            user_name: None,
            transaction: TransactionScope::None,
            shared_scheduler: None,
            partition_count: 0,
            arena: runtime.arena.clone(),
            feedback_history: None,
            columnar_policy: None,
            macro_manager: runtime.macro_manager.clone(),
            type_alias_manager: runtime.type_alias_manager.clone(),
            search: runtime.search.clone(),
        }
    }

    /// Reset per-run graph-algorithm state and rewind the input. All state
    /// is derived per input row, so only the input needs rewinding.
    pub fn reset(&mut self, input: &mut StreamingExecutor) -> Result<bool, QueryError> {
        input.reset()?;
        Ok(false)
    }
}
