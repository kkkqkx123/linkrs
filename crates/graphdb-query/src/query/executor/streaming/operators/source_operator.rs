use std::ops::Range;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::plan::types::PhysicalOperatorId;
use crate::query::executor::streaming::runtime::ExecutionRuntime;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::executor::streaming::state::{GlobalState, GlobalStateKey, StateArenaSet};
use crate::storage::EdgeCursor;
use crate::storage::IndexCursor;
use crate::storage::IndexRow;
use crate::storage::ScanPredicate;
use crate::storage::VertexCursor;

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
    /// Standalone DML values. Expressions are evaluated once in `open()`
    /// (i.e. once per execution) so volatile expressions such as `now()`
    /// are resolved at execution time.
    StandaloneValues {
        values: Vec<Vec<crate::core::types::expr::ContextualExpression>>,
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
        col_names: Vec<String>,
        projected_properties: Vec<String>,
        /// Scan predicates pushed into the storage layer.
        predicate: Vec<ScanPredicate>,
        /// Tag-restricted scan: only rows of this tag are scanned at the
        /// storage layer.
        tag: Option<String>,
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
        partition_range: Option<std::ops::Range<i64>>,
        col_names: Vec<String>,
        projected_properties: Vec<String>,
        /// Scan predicates pushed into the storage layer.
        predicate: Vec<ScanPredicate>,
        cursor: Option<Box<dyn EdgeCursor>>,
    },
    /// Fetch vertices by ID.
    GetVertices {
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        vertex_ids: Option<Vec<Value>>,
        cached_ids: Vec<VertexId>,
        projected_properties: Vec<String>,
    },
    /// Fetch edges by src/dst/type/rank.
    GetEdges {
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        edge_type: Option<String>,
        src: Option<String>,
        dst: Option<String>,
        rank: i64,
        projected_properties: Vec<String>,
        cursor: Option<Box<dyn EdgeCursor>>,
    },
    /// Traverse neighbors of each input vertex.
    GetNeighbors {
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        direction: String,
        projected_properties: Vec<String>,
        state: NeighborScanState,
    },
    /// Index scan with typed predicate and projection.
    IndexScan {
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        index_name: String,
        index_id: u64,
        predicate: BoundIndexPredicate,
        projection: IndexProjection,
        output_layout: Arc<SlotLayout>,
        partition_range: Option<Range<i64>>,
        cursor: Option<Box<dyn IndexCursor<Row = IndexRow>>>,
        edge_type_names: std::collections::HashMap<u32, String>,
    },
    Argument,
    /// Property retrieval (zero-input source, will migrate to Unary in M2).
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
            super::spec::SourceSpec::StorageScanVertices {
                space_name,
                limit,
                col_names,
                projected_properties,
                predicate,
                tag,
                partition_range,
            } => SourceOperatorKind::StorageScanVertices {
                storage: storage.clone(),
                space_name: space_name.clone(),
                limit: *limit,
                partition_range: partition_range.clone(),
                col_names: col_names.clone(),
                projected_properties: projected_properties.clone(),
                predicate: predicate.clone(),
                tag: tag.clone(),
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
                predicate,
                partition_range,
            } => SourceOperatorKind::StorageScanEdges {
                storage: storage.clone(),
                space_name: space_name.clone(),
                limit: *limit,
                edge_type: edge_type.clone(),
                partition_range: partition_range.clone(),
                col_names: col_names.clone(),
                projected_properties: projected_properties.clone(),
                predicate: predicate.clone(),
                cursor: None,
            },
            super::spec::SourceSpec::GetVertices {
                space_name,
                vertex_ids,
                projected_properties,
                col_names: _,
            } => SourceOperatorKind::GetVertices {
                storage: storage.clone(),
                space_name: space_name.clone(),
                vertex_ids: vertex_ids.clone(),
                cached_ids: Vec::new(),
                projected_properties: projected_properties.clone(),
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
                projected_properties: projected_properties.clone(),
                cursor: None,
            },
            super::spec::SourceSpec::GetNeighbors {
                space_name,
                direction,
                projected_properties,
            } => SourceOperatorKind::GetNeighbors {
                storage: storage.clone(),
                space_name: space_name.clone(),
                direction: direction.clone(),
                projected_properties: projected_properties.clone(),
                state: NeighborScanState::Init,
            },
            super::spec::SourceSpec::IndexScan {
                space_name,
                index_name,
                index_id,
                predicate,
                projection,
                output_layout,
                ..
            } => SourceOperatorKind::IndexScan {
                storage: storage.clone(),
                space_name: space_name.clone(),
                index_name: index_name.clone(),
                index_id: *index_id,
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
        use crate::query::executor::streaming::operators::state::SourceState;
        use crate::query::executor::streaming::state::GlobalState;
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
        use crate::query::executor::streaming::operators::state::SourceState;
        use crate::query::executor::streaming::state::GlobalState;
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
            SourceOperatorKind::GetProp { .. } => Err(QueryError::execution(
                "GetProp is not available as a source operator; \
                 use the unary GetProp (coming in M2)"
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
                                        Value::Null(crate::core::value::NullType::Null)
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
        use crate::query::executor::streaming::operators::state::SourceState;
        use crate::query::executor::streaming::state::GlobalState;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use crate::query::executor::base::MemoryBudget;
    use crate::query::executor::streaming::operators::base::OperatorBase;
    use crate::query::executor::streaming::runtime::{ExecutionRuntime, QueryIdentity};

    fn source(kind: SourceOperatorKind) -> SourceOperator {
        SourceOperator::new(kind, Arc::new(SlotLayout::new(Vec::new())))
    }

    fn config_for(base: &OperatorBase) -> OperatorConfig {
        OperatorConfig {
            chunk_size: base.chunk_size,
            partition_id: base.partition_id,
            physical_operator_id: base.physical_operator_id,
        }
    }

    #[test]
    fn scan_source_terminates_after_consuming_its_buffer() {
        let mut source = source(SourceOperatorKind::ScanVertices {
            buffer: vec![vec![Value::BigInt(1)]],
            current_index: 0,
            col_names: Vec::new(),
        });
        let base = OperatorBase::new(0);

        source.inject_context(base.runtime.as_ref(), config_for(&base));
        source.open().expect("source should open");
        assert_eq!(
            source
                .next()
                .expect("first pull should succeed")
                .map(|chunk| chunk.len()),
            Some(1)
        );
        assert!(source.next().expect("second pull should succeed").is_none());
    }

    #[test]
    fn scan_source_splits_across_multiple_chunks() {
        let base = OperatorBase::new(0);
        let chunk_size = base.chunk_size;
        let row_count = chunk_size * 2 + 7;
        let buffer: Vec<Vec<Value>> = (0..row_count)
            .map(|i| vec![Value::BigInt(i as i64)])
            .collect();
        let mut source = source(SourceOperatorKind::ScanVertices {
            buffer,
            current_index: 0,
            col_names: Vec::new(),
        });
        source.inject_context(base.runtime.as_ref(), config_for(&base));
        source.open().expect("source should open");

        let chunk1 = source
            .next()
            .expect("first pull should succeed")
            .expect("first chunk should be Some");
        assert_eq!(chunk1.len(), chunk_size);

        let chunk2 = source
            .next()
            .expect("second pull should succeed")
            .expect("second chunk should be Some");
        assert_eq!(chunk2.len(), chunk_size);

        let chunk3 = source
            .next()
            .expect("third pull should succeed")
            .expect("third chunk should be Some");
        assert_eq!(chunk3.len(), 7);

        assert!(source.next().expect("fourth pull should succeed").is_none());

        let total: i64 = chunk1
            .rows
            .iter()
            .chain(chunk2.rows.iter())
            .chain(chunk3.rows.iter())
            .map(|row| match &row[0] {
                Value::BigInt(n) => *n,
                _ => 0,
            })
            .sum();
        let expected: i64 = (0..row_count as i64).sum();
        assert_eq!(total, expected);
    }

    #[test]
    fn scan_without_col_names_infers_layout_from_row_width() {
        let base = OperatorBase::new(0);
        let mut source = source(SourceOperatorKind::ScanVertices {
            buffer: vec![vec![Value::BigInt(1), Value::string("a")]],
            current_index: 0,
            col_names: Vec::new(),
        });
        source.inject_context(base.runtime.as_ref(), config_for(&base));
        source.open().expect("source should open");
        let mut chunk = source
            .next()
            .expect("pull should succeed")
            .expect("chunk should be Some");
        assert_eq!(chunk.num_columns(), 2);
        assert_eq!(chunk.col_names(), vec!["c0", "c1"]);
        chunk.materialize_columns();
        assert_eq!(chunk.columns.as_deref().unwrap()[0].len(), 1);
    }

    #[test]
    fn buffered_scan_propagates_memory_budget_errors() {
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(0),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "vector")]
            None,
        ));
        let base = OperatorBase::new(0).with_runtime(Some(runtime));
        let mut source = source(SourceOperatorKind::ScanVertices {
            buffer: vec![vec![Value::string("row")]],
            current_index: 0,
            col_names: Vec::new(),
        });
        source.inject_context(base.runtime.as_ref(), config_for(&base));

        source.open().expect("source should open");
        let error = source
            .next()
            .expect_err("the source must propagate a memory budget error");
        assert!(error.to_string().contains("Memory budget exceeded"));
    }

    #[test]
    fn buffered_source_open_returns_configuration_errors() {
        let mut source = source(SourceOperatorKind::GetVertices {
            storage: None,
            space_name: "test".to_string(),
            vertex_ids: None,
            cached_ids: Vec::new(),
            projected_properties: Vec::new(),
        });
        let base = OperatorBase::new(0);
        source.inject_context(base.runtime.as_ref(), config_for(&base));

        let error = source.next().expect_err("source without storage must fail");
        assert!(error.to_string().contains("requires storage"));
        assert!(source.close().is_ok());
    }

    #[test]
    fn get_vertices_single_id_returns_none_on_second_call() {
        let mock = crate::storage::MockStorage::new().expect("MockStorage should be created");
        let storage = Arc::new(RwLock::new(mock));
        let mut source = source(SourceOperatorKind::GetVertices {
            storage: Some(storage),
            space_name: "test".to_string(),
            vertex_ids: Some(vec![Value::string("1")]),
            cached_ids: Vec::new(),
            projected_properties: Vec::new(),
        });
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(1024 * 1024),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "vector")]
            None,
        ));
        let base = OperatorBase::new(0).with_runtime(Some(runtime));
        source.inject_context(base.runtime.as_ref(), config_for(&base));

        source.open().expect("open should succeed");

        let result1 = source.next().expect("first next should succeed");
        assert!(
            result1.is_none(),
            "no vertex in mock, first call returns None"
        );

        let result2 = source.next().expect("second next should succeed");
        assert!(
            result2.is_none(),
            "second call must also return None (regression: do not re-emit)"
        );
    }

    #[test]
    fn get_edges_projected_filters_edge_properties() {
        use crate::core::Edge;
        let mock = crate::storage::MockStorage::new().expect("MockStorage should be created");
        let src = crate::core::types::storage_ids::VertexId::from_int64(1);
        let dst = crate::core::types::storage_ids::VertexId::from_int64(2);
        mock.set_edges(vec![Edge {
            src,
            dst,
            edge_type: "friend".to_string(),
            ranking: 0,
            props: vec![
                ("degree".to_string(), Value::Double(0.8)),
                ("since".to_string(), Value::Int(2020)),
            ]
            .into_iter()
            .collect(),
        }]);
        let storage = Arc::new(RwLock::new(mock));
        let mut source = source(SourceOperatorKind::GetEdges {
            storage: Some(storage),
            space_name: "test".to_string(),
            edge_type: Some("friend".to_string()),
            src: Some("1".to_string()),
            dst: Some("2".to_string()),
            rank: 0,
            projected_properties: vec!["degree".to_string()],
            cursor: None,
        });
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(1024 * 1024),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "vector")]
            None,
        ));
        let base = OperatorBase::new(0).with_runtime(Some(runtime));
        source.inject_context(base.runtime.as_ref(), config_for(&base));

        source.open().expect("open should succeed");

        let chunk = source
            .next()
            .expect("first next should succeed")
            .expect("edge must be returned");
        assert_eq!(chunk.len(), 1);
        let row = &chunk.rows[0];
        assert_eq!(row.len(), 2, "edge column + one flat property column");
        match &row[0] {
            Value::Edge(edge) => {
                assert_eq!(edge.properties().len(), 1, "only degree must be kept");
                assert_eq!(edge.properties().get("degree"), Some(&Value::Double(0.8)));
            }
            other => panic!("expected Value::Edge, got {:?}", other),
        }
        assert_eq!(row[1], Value::Double(0.8));

        assert!(source.next().expect("second next should succeed").is_none());
    }

    // ── Reset protocol ──

    #[test]
    fn buffered_scan_reset_rewinds_the_buffer() {
        let mut source = source(SourceOperatorKind::ScanVertices {
            buffer: vec![
                vec![Value::BigInt(1)],
                vec![Value::BigInt(2)],
                vec![Value::BigInt(3)],
            ],
            current_index: 0,
            col_names: Vec::new(),
        });
        let base = OperatorBase::new(0);
        source.inject_context(base.runtime.as_ref(), config_for(&base));

        source.open().expect("open should succeed");
        let mut all = Vec::new();
        while let Some(chunk) = source.next().expect("first run should succeed") {
            all.extend(chunk.rows);
        }
        assert_eq!(all.len(), 3, "first run emits every row");

        source
            .reset()
            .expect("reset should succeed without fallback");

        let mut again = Vec::new();
        while let Some(chunk) = source.next().expect("second run should succeed") {
            again.extend(chunk.rows);
        }
        assert_eq!(again, all, "reset re-produces the identical stream");
    }

    #[test]
    fn start_source_reset_remits_its_single_row() {
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(1024 * 1024),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "vector")]
            None,
        ));
        let base = OperatorBase::new(0).with_runtime(Some(runtime));
        let mut source = source(SourceOperatorKind::Start);
        source.inject_context(base.runtime.as_ref(), config_for(&base));

        source.open().expect("open should succeed");
        assert!(source.next().expect("pull").is_some());
        assert!(source.next().expect("pull").is_none());

        source.reset().expect("reset should succeed");
        assert!(
            source.next().expect("pull").is_some(),
            "reset un-emits Start"
        );
        assert!(source.next().expect("pull").is_none());
    }

    #[test]
    fn argument_source_reads_the_injected_frame_per_reset() {
        let layout = Arc::new(SlotLayout::from_names(&["id".to_string()]));
        let mut source = SourceOperator::new(SourceOperatorKind::Argument, layout.clone());

        source.open().expect("open should succeed");

        source.frame = Some((layout.clone(), vec![Value::Int(1)]));
        let chunk = source.next().expect("pull").expect("frame row emitted");
        assert_eq!(chunk.rows, vec![vec![Value::Int(1)]]);
        assert!(
            source.next().expect("pull").is_none(),
            "frame is consumed once per run"
        );

        source.reset().expect("reset should succeed");
        source.frame = Some((layout.clone(), vec![Value::Int(2)]));
        let chunk = source.next().expect("pull").expect("new frame row emitted");
        assert_eq!(chunk.rows, vec![vec![Value::Int(2)]]);
    }

    #[test]
    fn storage_backed_source_reset_reopens_and_repulls() {
        use crate::core::types::storage_ids::VertexId;
        use crate::core::Edge;
        use crate::storage::MockStorage;

        let mock = MockStorage::new().expect("MockStorage should be created");
        mock.set_edges(vec![Edge {
            src: VertexId::from_int64(1),
            dst: VertexId::from_int64(2),
            edge_type: "friend".to_string(),
            ranking: 0,
            props: Default::default(),
        }]);
        let storage: Arc<RwLock<dyn crate::storage::QueryStorage>> = Arc::new(RwLock::new(mock));
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(1024 * 1024),
            Some(storage.clone()),
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "vector")]
            None,
        ));
        let base = OperatorBase::new(0).with_runtime(Some(runtime));
        let mut source = source(SourceOperatorKind::GetEdges {
            storage: Some(storage),
            space_name: "test".to_string(),
            edge_type: Some("friend".to_string()),
            src: Some("1".to_string()),
            dst: Some("2".to_string()),
            rank: 0,
            projected_properties: Vec::new(),
            cursor: None,
        });
        source.inject_context(base.runtime.as_ref(), config_for(&base));

        source.open().expect("open should succeed");
        let first = source.next().expect("pull").expect("edge emitted");
        assert_eq!(first.len(), 1);
        assert!(source.next().expect("pull").is_none());

        source.reset().expect("reset should succeed");
        let second = source
            .next()
            .expect("pull")
            .expect("same edge re-emitted after reset");
        assert_eq!(second.len(), 1);
        assert!(source.next().expect("pull").is_none());
    }
}
