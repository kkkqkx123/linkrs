use std::ops::Range;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::storage_ids::VertexId;
use crate::core::Value;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::slot::SlotLayout;
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

/// Source operator with arena-based state for counters.
///
/// Heavy mutable resources (cursors) are kept inline for practical lifetime
/// management; simple counters and state machines live in the `SourceState`
/// arena on [`OperatorBase`].
#[derive(Debug)]
pub enum SourceOperator {
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

impl SourceOperator {
    /// Create a SourceOperator with immutable config from an immutable spec
    /// and the per-query storage client.  Mutable runtime state is created
    /// separately in [`SourceOperator::open`] and stored in the operator
    /// state arena on [`OperatorBase`].
    pub fn from_spec(
        spec: &super::spec::SourceSpec,
        storage: Option<Arc<RwLock<dyn crate::storage::QueryStorage>>>,
    ) -> Self {
        match spec {
            super::spec::SourceSpec::ScanVertices { rows, col_names } => Self::ScanVertices {
                buffer: rows.clone(),
                current_index: 0,
                col_names: col_names.clone(),
            },
            super::spec::SourceSpec::StandaloneValues { values, col_names } => {
                Self::StandaloneValues {
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
                partition_range,
            } => Self::StorageScanVertices {
                storage: storage.clone(),
                space_name: space_name.clone(),
                limit: *limit,
                partition_range: partition_range.clone(),
                col_names: col_names.clone(),
                projected_properties: projected_properties.clone(),
                predicate: predicate.clone(),
                cursor: None,
            },
            super::spec::SourceSpec::ScanEdges { rows, col_names } => Self::ScanEdges {
                buffer: rows.clone(),
                current_index: 0,
                col_names: col_names.clone(),
            },
            super::spec::SourceSpec::StorageScanEdges {
                space_name,
                limit,
                edge_type,
                col_names,
                projected_properties,
                partition_range,
            } => Self::StorageScanEdges {
                storage: storage.clone(),
                space_name: space_name.clone(),
                limit: *limit,
                edge_type: edge_type.clone(),
                partition_range: partition_range.clone(),
                col_names: col_names.clone(),
                projected_properties: projected_properties.clone(),
                cursor: None,
            },
            super::spec::SourceSpec::GetVertices {
                space_name,
                vertex_ids,
                projected_properties,
            } => Self::GetVertices {
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
            } => Self::GetEdges {
                storage: storage.clone(),
                space_name: space_name.clone(),
                edge_type: edge_type.clone(),
                src: src.clone(),
                dst: dst.clone(),
                rank: *rank,
                cursor: None,
            },
            super::spec::SourceSpec::GetNeighbors {
                space_name,
                direction,
                projected_properties,
            } => Self::GetNeighbors {
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
            } => Self::IndexScan {
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
            super::spec::SourceSpec::Argument => Self::Argument,
            super::spec::SourceSpec::GetProp {
                space_name,
                entity_slot,
                prop_names,
                is_vertex,
                output_layout,
            } => Self::GetProp {
                storage: storage.clone(),
                space_name: space_name.clone(),
                entity_slot: *entity_slot,
                prop_names: prop_names.clone(),
                is_vertex: *is_vertex,
                output_layout: output_layout.clone(),
            },
            super::spec::SourceSpec::Start => Self::Start,
        }
    }

    pub fn open(&mut self, base: &mut OperatorBase) -> Result<(), QueryError> {
        use crate::query::executor::streaming::operators::state::SourceState;
        use crate::query::executor::streaming::state::GlobalState;
        match self {
            Self::ScanVertices { .. } | Self::StandaloneValues { .. } | Self::ScanEdges { .. } => {
                buffered::open(self, base)?
            }
            Self::StorageScanVertices { .. } | Self::StorageScanEdges { .. } => {
                storage_scan::open(self, base)?
            }
            Self::GetVertices { .. } | Self::GetEdges { .. } => point_lookup::open(self, base)?,
            Self::GetNeighbors { .. } => neighbors::open(self, base)?,
            Self::IndexScan { .. } => index_scan::open(self, base)?,
            Self::Argument => base.insert_state(GlobalState::Source(SourceState::Argument)),
            Self::GetProp {
                entity_slot,
                prop_names,
                ..
            } => {
                base.insert_state(GlobalState::Source(SourceState::GetProp {
                    entity_slot: *entity_slot,
                    prop_names: prop_names.clone(),
                }));
            }
            Self::Start => {
                base.insert_state(GlobalState::Source(SourceState::Start { emitted: false }))
            }
        }
        base.lifecycle.mark_opened();
        Ok(())
    }

    pub fn next(&mut self, base: &mut OperatorBase) -> Result<Option<DataChunk>, QueryError> {
        use crate::query::executor::streaming::operators::state::SourceState;
        use crate::query::executor::streaming::state::GlobalState;
        match self {
            Self::ScanVertices { .. } | Self::StandaloneValues { .. } | Self::ScanEdges { .. } => {
                buffered::next(self, base)
            }
            Self::StorageScanVertices { .. } | Self::StorageScanEdges { .. } => {
                storage_scan::next(self, base)
            }
            Self::GetVertices { .. } | Self::GetEdges { .. } => point_lookup::next(self, base),
            Self::GetNeighbors { .. } => neighbors::next(self, base),
            Self::IndexScan { .. } => index_scan::next(self, base),
            Self::GetProp { .. } => Err(QueryError::execution(
                "GetProp is not available as a source operator; \
                 use the unary GetProp (coming in M2)"
                    .to_string(),
            )),
            Self::Start => {
                let mut arena = base.state_arena();
                let s = arena.global.get_mut(&base.state_key());
                let emitted = match s {
                    Some(GlobalState::Source(SourceState::Start { ref mut emitted })) => emitted,
                    _ => return Ok(None),
                };
                if *emitted {
                    return Ok(None);
                }
                *emitted = true;
                let chunk =
                    DataChunk::new_with_layout(vec![Vec::new()], Arc::clone(&base.output_layout));
                Ok(Some(chunk))
            }
            Self::Argument => {
                let rt = base.runtime.as_ref().ok_or_else(|| {
                    QueryError::execution(
                        "Argument requires a runtime with correlation frame".to_string(),
                    )
                })?;
                let frame = rt.take_correlation_frame();
                match frame {
                    Some((_layout, row)) => {
                        let chunk =
                            DataChunk::new_with_layout(vec![row], Arc::clone(&base.output_layout));
                        Ok(Some(chunk))
                    }
                    None => Ok(None),
                }
            }
        }
    }

    pub fn stop(&mut self, _base: &mut OperatorBase) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self, base: &mut OperatorBase) -> Result<(), QueryError> {
        base.take_state();
        base.lifecycle.mark_closed();
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

    #[test]
    fn scan_source_terminates_after_consuming_its_buffer() {
        let mut source = SourceOperator::ScanVertices {
            buffer: vec![vec![Value::BigInt(1)]],
            current_index: 0,
            col_names: Vec::new(),
        };
        let mut base = OperatorBase::new(0);

        source.open(&mut base).expect("source should open");
        assert_eq!(
            source
                .next(&mut base)
                .expect("first pull should succeed")
                .map(|chunk| chunk.len()),
            Some(1)
        );
        assert!(source
            .next(&mut base)
            .expect("second pull should succeed")
            .is_none());
    }

    #[test]
    fn scan_source_splits_across_multiple_chunks() {
        let mut base = OperatorBase::new(0);
        let chunk_size = base.chunk_size;
        let row_count = chunk_size * 2 + 7;
        let buffer: Vec<Vec<Value>> = (0..row_count)
            .map(|i| vec![Value::BigInt(i as i64)])
            .collect();
        let mut source = SourceOperator::ScanVertices {
            buffer,
            current_index: 0,
            col_names: Vec::new(),
        };
        source.open(&mut base).expect("source should open");

        let chunk1 = source
            .next(&mut base)
            .expect("first pull should succeed")
            .expect("first chunk should be Some");
        assert_eq!(chunk1.len(), chunk_size);

        let chunk2 = source
            .next(&mut base)
            .expect("second pull should succeed")
            .expect("second chunk should be Some");
        assert_eq!(chunk2.len(), chunk_size);

        let chunk3 = source
            .next(&mut base)
            .expect("third pull should succeed")
            .expect("third chunk should be Some");
        assert_eq!(chunk3.len(), 7);

        assert!(source
            .next(&mut base)
            .expect("fourth pull should succeed")
            .is_none());

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
        let mut base = OperatorBase::new(0);
        let mut source = SourceOperator::ScanVertices {
            buffer: vec![vec![Value::BigInt(1), Value::string("a")]],
            current_index: 0,
            col_names: Vec::new(),
        };
        source.open(&mut base).expect("source should open");
        let mut chunk = source
            .next(&mut base)
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
            #[cfg(feature = "qdrant")]
            None,
        ));
        let mut base = OperatorBase::new(0).with_runtime(Some(runtime));
        let mut source = SourceOperator::ScanVertices {
            buffer: vec![vec![Value::string("row")]],
            current_index: 0,
            col_names: Vec::new(),
        };

        source.open(&mut base).expect("source should open");
        let error = source
            .next(&mut base)
            .expect_err("the source must propagate a memory budget error");
        assert!(error.to_string().contains("Memory budget exceeded"));
    }

    #[test]
    fn buffered_source_open_returns_configuration_errors() {
        let mut source = SourceOperator::GetVertices {
            storage: None,
            space_name: "test".to_string(),
            vertex_ids: None,
            cached_ids: Vec::new(),
            projected_properties: Vec::new(),
        };
        let mut base = OperatorBase::new(0);

        let error = source
            .next(&mut base)
            .expect_err("source without storage must fail");
        assert!(error.to_string().contains("requires storage"));
        assert!(source.close(&mut base).is_ok());
    }

    #[test]
    fn get_vertices_single_id_returns_none_on_second_call() {
        let mock = crate::storage::MockStorage::new().expect("MockStorage should be created");
        let storage = Arc::new(RwLock::new(mock));
        let mut source = SourceOperator::GetVertices {
            storage: Some(storage),
            space_name: "test".to_string(),
            vertex_ids: Some(vec![Value::string("1")]),
            cached_ids: Vec::new(),
            projected_properties: Vec::new(),
        };
        let runtime = Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(1024 * 1024),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "qdrant")]
            None,
        ));
        let mut base = OperatorBase::new(0).with_runtime(Some(runtime));

        source.open(&mut base).expect("open should succeed");

        let result1 = source.next(&mut base).expect("first next should succeed");
        assert!(
            result1.is_none(),
            "no vertex in mock, first call returns None"
        );

        let result2 = source.next(&mut base).expect("second next should succeed");
        assert!(
            result2.is_none(),
            "second call must also return None (regression: do not re-emit)"
        );
    }
}
