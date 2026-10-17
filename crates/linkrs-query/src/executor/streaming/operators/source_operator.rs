use std::ops::Range;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::plan::types::PhysicalOperatorId;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use crate::executor::streaming::state::{GlobalState, GlobalStateKey, StateArenaSet};
use crate::storage::EdgeCursor;
use crate::storage::IndexCursor;
use crate::storage::IndexRow;
use crate::storage::ScanPredicate;
use crate::storage::ShardSlice;
use crate::storage::VertexCursor;
use linkrs_core::error::QueryError;
use linkrs_core::types::storage_ids::VertexId;
use linkrs_core::Value;

use super::spec::{BoundIndexPredicate, IndexProjection};

mod buffered;
mod index_scan;
mod neighbors;
mod point_lookup;
mod storage_scan;
mod util;

pub use neighbors::NeighborScanState;
pub use storage_scan::{column_block_enabled, set_column_block_enabled};

/// Immutable per-operator execution config injected at `open()`.
///
/// Mirrors the immutable fields of the executor's [`OperatorBase`] so the
/// operator hot path (in particular `next()`) never needs the base passed
/// down as a parameter.
#[derive(Debug, Clone, Copy)]
pub struct OperatorConfig {
    pub chunk_size: usize,
    pub partition_id: Option<usize>,
    pub physical_operator_id: PhysicalOperatorId,
}

impl Default for OperatorConfig {
    fn default() -> Self {
        Self {
            chunk_size: 2048,
            partition_id: None,
            physical_operator_id: PhysicalOperatorId(0),
        }
    }
}

/// Source operator kind with arena-based state for counters.
///
/// Heavy mutable resources (cursors) are kept inline for practical lifetime
/// management; simple counters and state machines live in the `SourceState`
/// arena on the execution runtime.
#[derive(Debug)]
pub enum SourceOperatorKind {
    /// Buffered vertex scan — rows come from the spec.
    ScanVertices {
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    /// Recursive-CTE working-table scan — rows are snapshotted from the
    /// enclosing fixpoint operator's published table at `open()`/`reset()`
    /// (once per step iteration) and served from the in-kind buffer.
    CteScan {
        cte_name: String,
        col_names: Vec<String>,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
    },
    /// Standalone DML values. Expressions are evaluated once in `open()`
    /// (i.e. once per execution) so volatile expressions such as `now()`
    /// are resolved at execution time.
    StandaloneValues {
        values: Vec<Vec<linkrs_core::types::expr::ContextualExpression>>,
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    /// Storage-backed vertex scan — rows come from a storage cursor.
    StorageScanVertices {
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        limit: Option<usize>,
        partition_range: Option<std::ops::Range<i64>>,
        /// Shard slice dividing enumeration across partitions (see the spec
        /// field): forwarded into the storage scan options at open.
        shard_slice: Option<crate::storage::ShardSlice>,
        col_names: Vec<String>,
        projected_properties: Vec<Arc<str>>,
        /// Identity mode (from the scan annotation): emit `Value::VertexId`
        /// in the entity column and skip the per-row property map.
        identity_only: bool,
        /// Rowless mode: skip the row view, emit typed columns only.
        /// Honored only when the columnar path is on.
        skip_rows: bool,
        /// Scan predicates pushed into the storage layer.
        predicate: Vec<ScanPredicate>,
        /// Tag-restricted scan: only rows of this tag are scanned at the
        /// storage layer.
        tag: Option<String>,
        /// Optional semi-mask pushed from join or expand: decode exactly
        /// these global internal ids instead of enumerating the table.
        semi_mask: Option<Vec<u32>>,
        cursor: Option<Box<dyn VertexCursor>>,
    },
    /// Buffered edge scan — rows come from the spec.
    ScanEdges {
        buffer: Vec<Vec<Value>>,
        current_index: usize,
        col_names: Vec<String>,
    },
    /// Storage-backed edge scan — rows come from a storage cursor.
    StorageScanEdges {
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        limit: Option<usize>,
        edge_type: Option<String>,
        shard_slice: Option<ShardSlice>,
        col_names: Vec<String>,
        projected_properties: Vec<Arc<str>>,
        /// Identity mode (from the scan annotation): emit
        /// `Value::EdgeHeader` in the entity column and skip the per-row
        /// property map.
        identity_only: bool,
        /// Scan predicates pushed into the storage layer.
        predicate: Vec<ScanPredicate>,
        cursor: Option<Box<dyn EdgeCursor>>,
    },
    /// Fetch vertices by ID.
    GetVertices {
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        tag: String,
        vertex_ids: Option<Vec<Value>>,
        cached_ids: Vec<VertexId>,
        projected_properties: Vec<Arc<str>>,
    },
    /// Fetch edges by src/dst/type/rank.
    GetEdges {
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        edge_type: Option<String>,
        src: Option<String>,
        dst: Option<String>,
        rank: i64,
        projected_properties: Vec<Arc<str>>,
        cursor: Option<Box<dyn EdgeCursor>>,
    },
    /// Traverse neighbors of each input vertex.
    GetNeighbors {
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        tag: String,
        direction: String,
        projected_properties: Vec<Arc<str>>,
        state: NeighborScanState,
    },
    /// Index scan with typed predicate and projection.
    IndexScan {
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        index_name: String,
        index_id: u64,
        tag: String,
        predicate: BoundIndexPredicate,
        projection: IndexProjection,
        output_layout: Arc<SlotLayout>,
        partition_range: Option<Range<i64>>,
        cursor: Option<Box<dyn IndexCursor<Row = IndexRow>>>,
        edge_type_names: std::collections::HashMap<u32, String>,
    },
    Argument,
    /// Property retrieval as a zero-input source (prefer the unary GetV).
    GetProp {
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        entity_slot: usize,
        prop_names: Vec<String>,
        is_vertex: bool,
        output_layout: Arc<SlotLayout>,
    },
    Start,
}

/// Source operator.
///
/// Wraps [`SourceOperatorKind`] with the runtime context injected at
/// `open()`. Lifecycle state is owned exclusively by the executor; operators
/// never write it.
#[derive(Debug)]
pub struct SourceOperator {
    pub kind: SourceOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
    /// Correlation frame bound to this operator instance by a parent
    /// executor (`StreamingExecutor::inject_correlation_frame`). Consumed
    /// (one-shot) by `Argument`. Per-instance, so parallel partitions and
    /// nested subqueries never share or overwrite frames.
    pub frame: Option<(Arc<SlotLayout>, Vec<Value>)>,
}

/// Property names reach the planner as owned `String`s, while every storage
/// read takes shared `Arc<str>` names. Converting once here keeps the
/// per-chunk read paths free of per-row name allocations.
fn shared_names(names: &[String]) -> Vec<Arc<str>> {
    names.iter().map(|name| Arc::from(name.as_str())).collect()
}

impl SourceOperator {
    /// Create a SourceOperator with immutable config from an immutable spec
    /// and the per-query storage client.  Mutable runtime state is created
    /// separately in [`SourceOperator::open`] and stored in the operator
    /// state arena on the execution runtime.
    pub fn from_spec(
        spec: &super::spec::SourceSpec,
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
        output_layout: Arc<SlotLayout>,
    ) -> Self {
        let kind = match spec {
            super::spec::SourceSpec::ScanVertices { rows, col_names } => {
                SourceOperatorKind::ScanVertices {
                    buffer: rows.clone(),
                    current_index: 0,
                    col_names: col_names.clone(),
                }
            }
            super::spec::SourceSpec::StandaloneValues { values, col_names } => {
                SourceOperatorKind::StandaloneValues {
                    values: values.clone(),
                    buffer: Vec::new(),
                    current_index: 0,
                    col_names: col_names.clone(),
                }
            }
            super::spec::SourceSpec::CteScan {
                cte_name,
                col_names,
            } => SourceOperatorKind::CteScan {
                cte_name: cte_name.clone(),
                col_names: col_names.clone(),
                buffer: Vec::new(),
                current_index: 0,
            },
            super::spec::SourceSpec::StorageScanVertices {
                space_name,
                limit,
                col_names,
                projected_properties,
                identity_only,
                skip_rows,
                predicate,
                tag,
                partition_range,
                shard_slice,
            } => SourceOperatorKind::StorageScanVertices {
                storage: storage.clone(),
                space_name: space_name.clone(),
                limit: *limit,
                partition_range: partition_range.clone(),
                shard_slice: *shard_slice,
                col_names: col_names.clone(),
                projected_properties: shared_names(projected_properties),
                identity_only: *identity_only,
                skip_rows: *skip_rows,
                predicate: predicate.clone(),
                tag: tag.clone(),
                semi_mask: None,
                cursor: None,
            },
            super::spec::SourceSpec::ScanEdges { rows, col_names } => {
                SourceOperatorKind::ScanEdges {
                    buffer: rows.clone(),
                    current_index: 0,
                    col_names: col_names.clone(),
                }
            }
            super::spec::SourceSpec::StorageScanEdges {
                space_name,
                limit,
                edge_type,
                col_names,
                projected_properties,
                identity_only,
                predicate,
                shard_slice,
            } => SourceOperatorKind::StorageScanEdges {
                storage: storage.clone(),
                space_name: space_name.clone(),
                limit: *limit,
                edge_type: edge_type.clone(),
                shard_slice: *shard_slice,
                col_names: col_names.clone(),
                projected_properties: shared_names(projected_properties),
                identity_only: *identity_only,
                predicate: predicate.clone(),
                cursor: None,
            },
            super::spec::SourceSpec::GetVertices {
                space_name,
                tag,
                vertex_ids,
                projected_properties,
                col_names: _,
            } => SourceOperatorKind::GetVertices {
                storage: storage.clone(),
                space_name: space_name.clone(),
                tag: tag.clone(),
                vertex_ids: vertex_ids.clone(),
                cached_ids: Vec::new(),
                projected_properties: shared_names(projected_properties),
            },
            super::spec::SourceSpec::GetEdges {
                space_name,
                edge_type,
                src,
                dst,
                rank,
                projected_properties,
            } => SourceOperatorKind::GetEdges {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_type: edge_type.clone(),
                src: src.clone(),
                dst: dst.clone(),
                rank: *rank,
                projected_properties: shared_names(projected_properties),
                cursor: None,
            },
            super::spec::SourceSpec::GetNeighbors {
                space_name,
                tag,
                direction,
                projected_properties,
            } => SourceOperatorKind::GetNeighbors {
                storage: storage.clone(),
                space_name: space_name.clone(),
                tag: tag.clone(),
                direction: direction.clone(),
                projected_properties: shared_names(projected_properties),
                state: NeighborScanState::Init,
            },
            super::spec::SourceSpec::IndexScan {
                space_name,
                index_name,
                index_id,
                tag,
                predicate,
                projection,
                output_layout,
                ..
            } => SourceOperatorKind::IndexScan {
                storage: storage.clone(),
                space_name: space_name.clone(),
                index_name: index_name.clone(),
                index_id: *index_id,
                tag: tag.clone(),
                predicate: (**predicate).clone(),
                projection: projection.clone(),
                output_layout: output_layout.clone(),
                partition_range: None,
                cursor: None,
                edge_type_names: std::collections::HashMap::new(),
            },
            super::spec::SourceSpec::Argument { .. } => SourceOperatorKind::Argument,
            super::spec::SourceSpec::GetProp {
                space_name,
                entity_slot,
                prop_names,
                is_vertex,
                output_layout,
            } => SourceOperatorKind::GetProp {
                storage: storage.clone(),
                space_name: space_name.clone(),
                entity_slot: *entity_slot,
                prop_names: prop_names.clone(),
                is_vertex: *is_vertex,
                output_layout: output_layout.clone(),
            },
            super::spec::SourceSpec::Start => SourceOperatorKind::Start,
        };
        Self {
            kind,
            runtime: None,
            output_layout,
            config: OperatorConfig {
                chunk_size: 2048,
                partition_id: None,
                physical_operator_id: PhysicalOperatorId(0),
            },
            frame: None,
        }
    }

    pub fn new(kind: SourceOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            kind,
            runtime: None,
            output_layout,
            config: OperatorConfig {
                chunk_size: 2048,
                partition_id: None,
                physical_operator_id: PhysicalOperatorId(0),
            },
            frame: None,
        }
    }

    /// Install a join or expand semi-mask on a vertex scan. The mask decodes
    /// exactly the given global internal ids instead of enumerating the
    /// table. Must be called before open; opening without a tag fails.
    pub fn set_semi_mask(&mut self, ids: Vec<u32>) {
        if let SourceOperatorKind::StorageScanVertices { semi_mask, .. } = &mut self.kind {
            *semi_mask = Some(ids);
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

    fn state_key(&self) -> GlobalStateKey {
        GlobalStateKey(self.config.physical_operator_id, self.config.partition_id)
    }

    fn state_arena(&self) -> parking_lot::MutexGuard<'_, StateArenaSet> {
        self.runtime
            .as_ref()
            .expect("runtime required")
            .state_arena_for(self.config.partition_id)
            .lock()
    }

    fn insert_state(&mut self, state: GlobalState) {
        let Some(rt) = self.runtime.as_ref() else {
            return;
        };
        let key = self.state_key();
        rt.state_arena_for(self.config.partition_id)
            .lock()
            .global
            .insert(key, state);
    }

    fn take_state(&mut self) -> Option<GlobalState> {
        let rt = self.runtime.as_ref()?;
        let key = self.state_key();
        rt.state_arena_for(self.config.partition_id)
            .lock()
            .global
            .remove(&key)
    }

    pub fn open(&mut self) -> Result<(), QueryError> {
        use crate::executor::streaming::operators::state::SourceState;
        use crate::executor::streaming::state::GlobalState;
        let mut state: Option<GlobalState> = None;
        match &mut self.kind {
            SourceOperatorKind::ScanVertices { .. }
            | SourceOperatorKind::StandaloneValues { .. }
            | SourceOperatorKind::ScanEdges { .. } => buffered::open(self)?,
            SourceOperatorKind::StorageScanVertices { .. }
            | SourceOperatorKind::StorageScanEdges { .. } => storage_scan::open(self)?,
            SourceOperatorKind::GetVertices { .. } | SourceOperatorKind::GetEdges { .. } => {
                point_lookup::open(self)?
            }
            SourceOperatorKind::GetNeighbors { .. } => neighbors::open(self)?,
            SourceOperatorKind::IndexScan { .. } => index_scan::open(self)?,
            SourceOperatorKind::CteScan { .. } => Self::open_cte_scan(self)?,
            SourceOperatorKind::Argument => {
                state = Some(GlobalState::Source(SourceState::Argument));
            }
            SourceOperatorKind::GetProp {
                entity_slot,
                prop_names,
                ..
            } => {
                state = Some(GlobalState::Source(SourceState::GetProp {
                    entity_slot: *entity_slot,
                    prop_names: prop_names.clone(),
                }));
            }
            SourceOperatorKind::Start => {
                state = Some(GlobalState::Source(SourceState::Start { emitted: false }));
            }
        }
        if let Some(state) = state {
            self.insert_state(state);
        }
        Ok(())
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<DataChunk>, QueryError> {
        use crate::executor::streaming::operators::state::SourceState;
        use crate::executor::streaming::state::GlobalState;
        match &mut self.kind {
            SourceOperatorKind::ScanVertices { .. }
            | SourceOperatorKind::StandaloneValues { .. }
            | SourceOperatorKind::ScanEdges { .. } => buffered::next(self),
            SourceOperatorKind::StorageScanVertices { .. }
            | SourceOperatorKind::StorageScanEdges { .. } => storage_scan::next(self),
            SourceOperatorKind::GetVertices { .. } | SourceOperatorKind::GetEdges { .. } => {
                point_lookup::next(self)
            }
            SourceOperatorKind::GetNeighbors { .. } => neighbors::next(self),
            SourceOperatorKind::IndexScan { .. } => index_scan::next(self),
            SourceOperatorKind::CteScan { .. } => Self::next_cte_scan(self),
            SourceOperatorKind::GetProp { .. } => Err(QueryError::execution(
                "GetProp is not available as a source operator; \
                  use the unary GetProp operator"
                    .to_string(),
            )),
            SourceOperatorKind::Start => {
                let mut arena = self.state_arena();
                let s = arena.global.get_mut(&self.state_key());
                let emitted = match s {
                    Some(GlobalState::Source(SourceState::Start { ref mut emitted })) => emitted,
                    _ => return Ok(None),
                };
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                let chunk =
                    DataChunk::new_with_layout(vec![Vec::new()], Arc::clone(&self.output_layout));
                Ok(Some(chunk))
            }
            SourceOperatorKind::Argument => {
                let frame = self.frame.take();
                match frame {
                    Some((layout, row)) => {
                        // Project the injected frame down to this Argument's
                        // own output layout: the injected row may carry more
                        // columns than the outer column names the sub-plan
                        // was compiled against (e.g. optimizer-added flat
                        // property slots).
                        let projected: Vec<Value> = self
                            .output_layout
                            .names()
                            .iter()
                            .map(|name| {
                                layout
                                    .slot_id(name)
                                    .and_then(|slot| row.get(slot).cloned())
                                    .unwrap_or_else(|| {
                                        Value::Null(linkrs_core::value::NullType::Null)
                                    })
                            })
                            .collect();
                        let chunk = DataChunk::new_with_layout(
                            vec![projected],
                            Arc::clone(&self.output_layout),
                        );
                        Ok(Some(chunk))
                    }
                    None => Ok(None),
                }
            }
        }
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    /// Rewind this source so it re-produces the same logical stream.
    ///
    /// Buffered sources reset their row index; storage-backed sources
    /// re-open their cursor (the query-level snapshot guarantees identical
    /// re-reads); `Argument`/`GetProp` keep their injected frame/state and
    /// `Start` un-emits its single row.
    pub fn reset(&mut self) -> Result<bool, QueryError> {
        use crate::executor::streaming::operators::state::SourceState;
        use crate::executor::streaming::state::GlobalState;
        match &mut self.kind {
            SourceOperatorKind::ScanVertices { current_index, .. }
            | SourceOperatorKind::ScanEdges { current_index, .. }
            | SourceOperatorKind::StandaloneValues { current_index, .. } => {
                *current_index = 0;
            }
            SourceOperatorKind::StorageScanVertices { .. }
            | SourceOperatorKind::StorageScanEdges { .. } => {
                storage_scan::open(self)?;
            }
            SourceOperatorKind::GetVertices { .. } | SourceOperatorKind::GetEdges { .. } => {
                point_lookup::open(self)?
            }
            SourceOperatorKind::GetNeighbors { .. } => neighbors::open(self)?,
            SourceOperatorKind::IndexScan { .. } => index_scan::open(self)?,
            SourceOperatorKind::CteScan { .. } => {
                // Re-snapshot the working table: the fixpoint operator
                // publishes a fresh delta before every step iteration.
                Self::open_cte_scan(self)?;
            }
            SourceOperatorKind::Argument => {
                // The correlation frame lives on the operator and is
                // re-injected per run by the parent; nothing to rewind.
            }
            SourceOperatorKind::GetProp { .. } => {
                // State is immutable; nothing to rewind.
            }
            SourceOperatorKind::Start => {
                if let Some(rt) = &self.runtime {
                    let key = self.state_key();
                    let mut arena = rt.state_arena_for(self.config.partition_id).lock();
                    if let Some(GlobalState::Source(SourceState::Start { emitted })) =
                        arena.global.get_mut(&key)
                    {
                        *emitted = false;
                    }
                }
            }
        }
        Ok(false)
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        self.take_state();
        Ok(())
    }

    /// Snapshot the enclosing fixpoint's working table into the in-kind
    /// buffer. Called at `open()` and at every `reset()` (once per step
    /// iteration); a missing table reads as an empty stream.
    fn open_cte_scan(op: &mut SourceOperator) -> Result<(), QueryError> {
        let (cte_name, buffer, current_index) = match &mut op.kind {
            SourceOperatorKind::CteScan {
                cte_name,
                buffer,
                current_index,
                ..
            } => (cte_name.clone(), buffer, current_index),
            _ => {
                return Err(QueryError::execution(
                    "open_cte_scan called for a non-CTE source".to_string(),
                ));
            }
        };
        let rows = op
            .runtime
            .as_ref()
            .and_then(|rt| rt.cte_table(&cte_name))
            .unwrap_or_default();
        *buffer = rows;
        *current_index = 0;
        Ok(())
    }

    /// Serve the next chunk from the snapshotted working table.
    fn next_cte_scan(op: &mut SourceOperator) -> Result<Option<DataChunk>, QueryError> {
        let layout = Arc::clone(&op.output_layout);
        let chunk_size = op.config.chunk_size;
        let (buffer, current_index) = match &mut op.kind {
            SourceOperatorKind::CteScan {
                buffer,
                current_index,
                ..
            } => (buffer, current_index),
            _ => {
                return Err(QueryError::execution(
                    "next_cte_scan called for a non-CTE source".to_string(),
                ));
            }
        };
        if *current_index >= buffer.len() {
            return Ok(None);
        }
        let end = (*current_index + chunk_size).min(buffer.len());
        let rows = buffer[*current_index..end].to_vec();
        *current_index = end;
        Ok(Some(DataChunk::new_with_layout(rows, layout)))
    }
}

#[cfg(test)]
#[cfg(test)]
mod tests;
