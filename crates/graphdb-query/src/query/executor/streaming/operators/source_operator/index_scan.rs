use std::ops::Range;
use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::types::MAX_TIMESTAMP;
use crate::core::wal::EntityRef;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::state::SourceState;
use crate::query::executor::streaming::slot::SlotLayout;
use crate::query::executor::streaming::state::GlobalState;
use crate::storage::{open_index_cursor, IndexCursor, IndexPredicate, IndexRow, IndexScanPlan, QueryStorage};
use parking_lot::RwLock;

use super::SourceOperator;
use super::super::spec::{BoundIndexPredicate, IndexProjection};
use super::util::{
    attach_columnar_stats, make_flat_covering_edge_row, make_flat_covering_vertex_row,
    make_flat_edge_row, make_flat_vertex_row, reserve_memory, storage_error,
};

/// Open the `IndexScan` source: build the physical scan plan and open the
/// cursor.
pub(crate) fn open(op: &mut SourceOperator, base: &mut OperatorBase) -> Result<(), QueryError> {
    match op {
        SourceOperator::IndexScan {
            storage,
            space_name,
            index_id,
            predicate,
            projection,
            partition_range,
            cursor,
            ..
        } => {
            let storage_ref = storage.as_ref().ok_or_else(|| {
                QueryError::execution("IndexScan requires storage".to_string())
            })?;
            let plan = build_index_scan_plan(
                storage_ref,
                space_name,
                *index_id,
                predicate,
                projection,
                partition_range.clone(),
            )?;
            *cursor = Some(open_index_cursor(storage_ref, &plan).map_err(|error| {
                storage_error("IndexScan", "open cursor", space_name, error)
            })?);
            base.insert_state(GlobalState::Source(SourceState::IndexScan { cursor: None }));
        }
        _ => unreachable!("index_scan::open called for a non-index source"),
    }
    Ok(())
}

/// Emit the next chunk from the index cursor, resolving row IDs back to
/// entities and handling covering rows.
pub(crate) fn next(
    op: &mut SourceOperator,
    base: &mut OperatorBase,
) -> Result<Option<DataChunk>, QueryError> {
    match op {
        SourceOperator::IndexScan {
            storage,
            space_name,
            output_layout,
            projection,
            cursor,
            edge_type_names,
            ..
        } => {
            let vertex_projection = match projection {
                IndexProjection::Columns(cols) => cols.clone(),
                _ => Vec::new(),
            };
            let storage = storage.as_ref().ok_or_else(|| {
                QueryError::execution("IndexScan requires storage".to_string())
            })?;
            let context = IndexScanContext {
                storage,
                space_name,
                edge_type_names,
            };
            next_index_chunk(
                context,
                cursor,
                output_layout,
                base,
                "IndexScan",
                &vertex_projection,
            )
        }
        _ => unreachable!("index_scan::next called for a non-index source"),
    }
}

fn build_index_scan_plan(
    storage: &Arc<RwLock<dyn QueryStorage>>,
    space_name: &str,
    index_id: u64,
    predicate: &BoundIndexPredicate,
    projection: &IndexProjection,
    partition_range: Option<Range<i64>>,
) -> Result<IndexScanPlan, QueryError> {
    let physical_predicate = match predicate {
        BoundIndexPredicate::Equal { value, .. } => IndexPredicate::Equal(value.clone()),
        BoundIndexPredicate::Range {
            begin,
            end,
            include_begin,
            include_end,
            ..
        } => IndexPredicate::Range {
            lower: begin.clone(),
            upper: end.clone(),
            include_lower: *include_begin,
            include_upper: *include_end,
        },
        BoundIndexPredicate::Prefix { prefix, .. } => IndexPredicate::Prefix(prefix.clone()),
        BoundIndexPredicate::Full => IndexPredicate::All,
    };

    let projection = match projection {
        IndexProjection::RowIdOnly => None,
        IndexProjection::Columns(columns) => Some(columns.clone()),
        IndexProjection::AllColumns => Some(Vec::new()),
    };
    let read_timestamp = storage
        .read()
        .operation_context()
        .map(|context| context.read_timestamp)
        .unwrap_or(MAX_TIMESTAMP);

    let partition_id_range = partition_range;

    Ok(IndexScanPlan {
        space: space_name.to_string(),
        index_id,
        predicate: physical_predicate,
        partition: graphdb_storage::storage::PartitionSelector::All,
        partition_id_range,
        projection,
        limit: None,
        offset: 0,
        read_timestamp,
    })
}

/// Shared read context for index-scan chunk production.
pub(crate) struct IndexScanContext<'a> {
    pub(crate) storage: &'a Arc<RwLock<dyn QueryStorage>>,
    pub(crate) space_name: &'a str,
    pub(crate) edge_type_names: &'a mut std::collections::HashMap<u32, String>,
}

fn next_index_chunk(
    context: IndexScanContext<'_>,
    cursor: &mut Option<Box<dyn IndexCursor<Row = IndexRow>>>,
    output_layout: &Arc<SlotLayout>,
    base: &mut OperatorBase,
    source: &str,
    vertex_projection: &[String],
) -> Result<Option<DataChunk>, QueryError> {
    let IndexScanContext {
        storage,
        space_name,
        edge_type_names,
    } = context;
    loop {
        base.ensure_not_cancelled()?;
        let mut index_cursor = match cursor.take() {
            Some(cursor) => cursor,
            None => return Ok(None),
        };
        let rows = index_cursor
            .next_batch(base.chunk_size)
            .map_err(|error| storage_error(source, "read cursor", space_name, error))?;
        let exhausted = index_cursor.is_exhausted();
        let mut output_rows = Vec::with_capacity(rows.len());

        if !rows.is_empty() {
            let guard = storage.read();
            for row in rows {
                match row {
                    IndexRow::Covering {
                        entity_ref,
                        columns,
                    } => match &entity_ref {
                        EntityRef::Vertex(_) => {
                            if let Some(row) =
                                make_flat_covering_vertex_row(&entity_ref, columns, vertex_projection)
                            {
                                output_rows.push(row);
                            }
                        }
                        EntityRef::Edge { edge_type, .. } => {
                            let Some(name) = resolve_edge_type_name(
                                &*guard,
                                space_name,
                                *edge_type,
                                edge_type_names,
                                source,
                            )?
                            else {
                                continue;
                            };
                            if let Some(row) = make_flat_covering_edge_row(
                                &entity_ref,
                                columns,
                                name,
                                vertex_projection,
                            ) {
                                output_rows.push(row);
                            }
                        }
                    },
                    IndexRow::RowId(entity_ref) => match &entity_ref {
                        EntityRef::Vertex(vid) => {
                            let result = if vertex_projection.is_empty() {
                                guard.get_vertex(space_name, vid)
                            } else {
                                guard.get_vertex_projected(space_name, vid, vertex_projection)
                            };
                            match result {
                                Ok(Some(vertex)) => output_rows
                                    .push(make_flat_vertex_row(vertex, vertex_projection)),
                                Ok(None) => {
                                    debug_assert!(
                                        false,
                                        "cursor yielded stale vertex {} in space {}",
                                        vid, space_name
                                    );
                                }
                                Err(error) => {
                                    return Err(storage_error(
                                        source,
                                        "get indexed vertex",
                                        space_name,
                                        error,
                                    ));
                                }
                            }
                        }
                        EntityRef::Edge {
                            src,
                            dst,
                            edge_type,
                            ranking,
                        } => {
                            let Some(name) = resolve_edge_type_name(
                                &*guard,
                                space_name,
                                *edge_type,
                                edge_type_names,
                                source,
                            )?
                            else {
                                continue;
                            };
                            match guard.get_edge(space_name, src, dst, &name, *ranking) {
                                Ok(Some(edge)) => output_rows
                                    .push(make_flat_edge_row(edge, vertex_projection)),
                                Ok(None) => {
                                    debug_assert!(
                                        false,
                                        "cursor yielded stale edge {} -> {} {}@{} in space {}",
                                        src, dst, name, ranking, space_name
                                    );
                                }
                                Err(error) => {
                                    return Err(storage_error(
                                        source,
                                        "get indexed edge",
                                        space_name,
                                        error,
                                    ));
                                }
                            }
                        }
                    },
                }
            }
        }

        *cursor = Some(index_cursor);
        if !output_rows.is_empty() {
            let reservation = reserve_memory(base, &output_rows)?;
            let chunk = attach_columnar_stats(
                base,
                DataChunk::new_with_layout(output_rows, output_layout.clone()),
            );
            let chunk = if let Some(reservation) = reservation {
                chunk.with_memory_reservation(reservation)
            } else {
                chunk
            };
            return Ok(Some(chunk));
        }
        if exhausted {
            return Ok(None);
        }
    }
}

/// Resolve an edge type name from its storage hash, using a per-query cache
/// so each distinct hash is resolved against the schema at most once.
pub(crate) fn resolve_edge_type_name(
    storage: &dyn QueryStorage,
    space_name: &str,
    hash: u32,
    cache: &mut std::collections::HashMap<u32, String>,
    source: &str,
) -> Result<Option<String>, QueryError> {
    if let Some(name) = cache.get(&hash) {
        return Ok(Some(name.clone()));
    }
    match storage.resolve_edge_type_name(space_name, hash) {
        Ok(Some(name)) => {
            cache.insert(hash, name.clone());
            Ok(Some(name))
        }
        Ok(None) => Ok(None),
        Err(error) => Err(storage_error(
            source,
            "resolve edge type",
            space_name,
            error,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::storage_ids::VertexId;
    use crate::core::types::EdgeTypeInfo;
    use crate::core::Edge;
    use crate::core::Value;
    use crate::query::executor::base::MemoryBudget;
    use crate::query::executor::streaming::operators::base::OperatorBase;
    use crate::query::executor::streaming::runtime::{ExecutionRuntime, QueryIdentity};
    use crate::storage::{IndexRow, MockStorage, StorageError};

    /// FNV-1a matching the storage index write path (`stable_hash`).
    fn edge_type_hash(name: &str) -> u32 {
        let mut hash = 0xcbf29ce484222325u64;
        for byte in name.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash as u32
    }

    fn knows_edge_entity_ref(ranking: i64) -> EntityRef {
        EntityRef::Edge {
            src: VertexId::from_int64(1),
            dst: VertexId::from_int64(2),
            edge_type: edge_type_hash("KNOWS"),
            ranking,
        }
    }

    #[derive(Debug)]
    struct FakeIndexCursor {
        rows: std::vec::IntoIter<IndexRow>,
        exhausted: bool,
    }

    impl FakeIndexCursor {
        fn new(rows: Vec<IndexRow>) -> Self {
            Self {
                rows: rows.into_iter(),
                exhausted: false,
            }
        }
    }

    impl IndexCursor for FakeIndexCursor {
        type Row = IndexRow;

        fn next_batch(&mut self, _batch_size: usize) -> Result<Vec<IndexRow>, StorageError> {
            if self.exhausted {
                return Ok(Vec::new());
            }
            self.exhausted = true;
            Ok(self.rows.by_ref().collect())
        }

        fn is_exhausted(&self) -> bool {
            self.exhausted
        }
    }

    fn test_runtime() -> Arc<ExecutionRuntime> {
        Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(1024 * 1024),
            None,
            #[cfg(feature = "fulltext-search")]
            None,
            #[cfg(feature = "qdrant")]
            None,
        ))
    }

    #[test]
    fn covering_index_edge_rows_do_not_require_a_table_fetch() {
        let row = make_flat_covering_edge_row(
            &knows_edge_entity_ref(7),
            vec![("since".to_string(), Value::Int(2024))],
            "KNOWS".to_string(),
            &[],
        )
        .expect("edge entity should produce a covering row");
        let Value::Edge(edge) = &row[0] else {
            panic!("covering row should contain an edge");
        };
        assert_eq!(edge.src(), &VertexId::from_int64(1));
        assert_eq!(edge.dst(), &VertexId::from_int64(2));
        assert_eq!(edge.edge_type(), "KNOWS");
        assert_eq!(edge.ranking(), 7);
        assert_eq!(edge.get_property("since"), Some(&Value::Int(2024)));
    }

    #[test]
    fn index_scan_resolves_edge_row_ids_back_to_edges() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("MockStorage should be created"),
        ));
        storage
            .write()
            .set_edge_types(vec![EdgeTypeInfo::new("KNOWS".to_string())]);
        storage.write().set_edges(vec![Edge::new(
            VertexId::from_int64(1),
            VertexId::from_int64(2),
            "KNOWS".to_string(),
            7,
            vec![("since".to_string(), Value::Int(2024))]
                .into_iter()
                .collect(),
        )]);

        let mut source = SourceOperator::IndexScan {
            storage: Some(storage),
            space_name: "test".to_string(),
            index_name: "knows_idx".to_string(),
            index_id: 1,
            predicate: BoundIndexPredicate::Equal {
                column: "since".to_string(),
                value: Value::Int(2024),
            },
            projection: IndexProjection::RowIdOnly,
            output_layout: Arc::new(SlotLayout::from_names(&["KNOWS".to_string()])),
            partition_range: None,
            cursor: Some(Box::new(FakeIndexCursor::new(vec![IndexRow::RowId(
                knows_edge_entity_ref(7),
            )]))),
            edge_type_names: std::collections::HashMap::new(),
        };
        let mut base = OperatorBase::new(0).with_runtime(Some(test_runtime()));

        let chunk = source
            .next(&mut base)
            .expect("pull should succeed")
            .expect("chunk should be Some");
        assert_eq!(chunk.len(), 1);
        let Value::Edge(edge) = &chunk.rows[0][0] else {
            panic!("row should contain an edge");
        };
        assert_eq!(edge.edge_type(), "KNOWS");
        assert_eq!(edge.ranking(), 7);
        assert_eq!(edge.get_property("since"), Some(&Value::Int(2024)));
    }

    #[test]
    fn index_scan_resolves_covering_edge_rows() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("MockStorage should be created"),
        ));
        storage
            .write()
            .set_edge_types(vec![EdgeTypeInfo::new("KNOWS".to_string())]);

        let mut source = SourceOperator::IndexScan {
            storage: Some(storage),
            space_name: "test".to_string(),
            index_name: "knows_idx".to_string(),
            index_id: 1,
            predicate: BoundIndexPredicate::Equal {
                column: "since".to_string(),
                value: Value::Int(2024),
            },
            projection: IndexProjection::Columns(vec!["since".to_string()]),
            output_layout: Arc::new(SlotLayout::from_names(&[
                "KNOWS".to_string(),
                "KNOWS.since".to_string(),
            ])),
            partition_range: None,
            cursor: Some(Box::new(FakeIndexCursor::new(vec![IndexRow::Covering {
                entity_ref: knows_edge_entity_ref(3),
                columns: vec![("since".to_string(), Value::Int(2024))],
            }]))),
            edge_type_names: std::collections::HashMap::new(),
        };
        let mut base = OperatorBase::new(0).with_runtime(Some(test_runtime()));

        let chunk = source
            .next(&mut base)
            .expect("pull should succeed")
            .expect("chunk should be Some");
        assert_eq!(chunk.len(), 1);
        let Value::Edge(edge) = &chunk.rows[0][0] else {
            panic!("row should contain an edge");
        };
        assert_eq!(edge.edge_type(), "KNOWS");
        assert_eq!(edge.ranking(), 3);
        assert_eq!(edge.get_property("since"), Some(&Value::Int(2024)));
    }

    #[test]
    fn index_scan_resolves_edge_type_name_from_cache() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("MockStorage should be created"),
        ));
        storage
            .write()
            .set_edge_types(vec![EdgeTypeInfo::new("KNOWS".to_string())]);

        let mut cache = std::collections::HashMap::new();
        let guard = storage.read();
        let name = resolve_edge_type_name(
            &*guard,
            "test",
            edge_type_hash("KNOWS"),
            &mut cache,
            "IndexScan",
        )
        .expect("resolve should succeed")
        .expect("KNOWS must resolve");
        assert_eq!(name, "KNOWS");
        assert!(cache.contains_key(&edge_type_hash("KNOWS")));

        let cached = resolve_edge_type_name(
            &*guard,
            "test",
            edge_type_hash("KNOWS"),
            &mut cache,
            "IndexScan",
        )
        .expect("cached resolve should succeed")
        .expect("cached name must resolve");
        assert_eq!(cached, "KNOWS");

        let missing = resolve_edge_type_name(&*guard, "test", 0xdead_beef, &mut cache, "IndexScan")
            .expect("missing resolve should succeed");
        assert!(missing.is_none());
    }

    #[test]
    fn covering_index_rows_do_not_require_a_table_fetch() {
        let row = make_flat_covering_vertex_row(
            &EntityRef::Vertex(VertexId::from_int64(7)),
            vec![("name".to_string(), Value::string("Alice"))],
            &[],
        )
        .expect("vertex entity should produce a covering row");
        let Value::Vertex(vertex) = &row[0] else {
            panic!("covering row should contain a vertex");
        };
        assert_eq!(vertex.vid.as_int64(), Some(7));
        assert_eq!(
            vertex.get_property_any("name"),
            Some(&Value::string("Alice"))
        );
    }

    #[test]
    fn covering_index_vertex_rows_emit_flat_property_columns() {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("MockStorage should be created"),
        ));
        let mut source = SourceOperator::IndexScan {
            storage: Some(storage),
            space_name: "test".to_string(),
            index_name: "person_idx".to_string(),
            index_id: 1,
            predicate: BoundIndexPredicate::Equal {
                column: "name".to_string(),
                value: Value::string("Alice"),
            },
            projection: IndexProjection::Columns(vec!["name".to_string(), "age".to_string()]),
            output_layout: Arc::new(SlotLayout::from_names(&[
                "v".to_string(),
                "v.name".to_string(),
                "v.age".to_string(),
            ])),
            partition_range: None,
            cursor: Some(Box::new(FakeIndexCursor::new(vec![
                IndexRow::Covering {
                    entity_ref: EntityRef::Vertex(VertexId::from_int64(7)),
                    columns: vec![
                        ("name".to_string(), Value::string("Alice")),
                        ("age".to_string(), Value::BigInt(30)),
                    ],
                },
            ]))),
            edge_type_names: std::collections::HashMap::new(),
        };
        let mut base = OperatorBase::new(0).with_runtime(Some(test_runtime()));

        let chunk = source
            .next(&mut base)
            .expect("pull should succeed")
            .expect("chunk should be Some");
        assert_eq!(chunk.len(), 1);
        assert_eq!(chunk.rows[0].len(), 3);
        assert!(matches!(&chunk.rows[0][0], Value::Vertex(_)));
        assert_eq!(chunk.rows[0][1], Value::string("Alice"));
        assert_eq!(chunk.rows[0][2], Value::BigInt(30));
        assert_eq!(chunk.col_names(), vec!["v", "v.name", "v.age"]);
    }
}