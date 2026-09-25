use super::CsrWithProperties;
use crate::cursor::{PredicateRange, ScanPredicate};
use crate::vertex::column::zone_map::ZONE_MAP_CHUNK_ROWS;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::Value;
use std::collections::HashSet;

type ProjectedProps = Vec<(String, Option<Value>)>;
type ProjectedBatch = Vec<Option<ProjectedProps>>;

impl CsrWithProperties {
    /// Read the property row for `edge_id` at `query_ts`, decoding only the
    /// projected columns.
    ///
    /// `projection` selects which columns to decode: `None` decodes every
    /// column, `Some(&[])` decodes none (topology-only read). Unknown names
    /// are skipped. Visibility is still enforced: an invisible edge yields
    /// `None`, a visible one yields `Some` (possibly empty).
    /// Test-only row-stamp filtered read; production uses physical read plus authority gate.
    #[cfg(test)]
    pub fn get_projected_by_edge_id(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        let pos = self.mapped_row(edge_id)?;
        let vis = self.visibility.get(pos)?;
        if !vis.is_visible_at(query_ts) {
            return None;
        }
        Some(
            self.resolve_projection(projection)
                .into_iter()
                .map(|(i, name)| {
                    let v = self.property_columns[i].get_at_ts(pos, query_ts);
                    (name.to_string(), v)
                })
                .collect(),
        )
    }

    /// Test-only row-stamp filtered read; production uses physical read plus authority gate.
    #[cfg(test)]
    pub fn get_by_edge_id(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
    ) -> Option<Vec<(String, Option<Value>)>> {
        self.get_projected_by_edge_id(edge_id, query_ts, None)
    }

    /// Resolve a projection to `(column position, name)` pairs in schema order.
    ///
    /// One hash lookup per requested name instead of one string scan per
    /// schema column, so wide schemas never pay a quadratic match and the
    /// hot batch path performs no string comparison at all. Unknown names
    /// are skipped and an empty list resolves to no columns, matching the
    /// historical filter semantics exactly. The pairs borrow the schema, so
    /// resolution itself allocates no name strings; only materialized output
    /// cells clone their column name.
    fn resolve_projection<'s>(&'s self, projection: Option<&[String]>) -> Vec<(usize, &'s str)> {
        match projection {
            None => self
                .property_schema
                .iter()
                .enumerate()
                .map(|(i, s)| (i, s.name.as_str()))
                .collect(),
            Some(names) => {
                if names.is_empty() {
                    return Vec::new();
                }
                let wanted: HashSet<&str> = names.iter().map(|n| n.as_str()).collect();
                self.property_schema
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| wanted.contains(s.name.as_str()))
                    .map(|(i, s)| (i, s.name.as_str()))
                    .collect()
            }
        }
    }

    /// Physical property projection without row visibility filtering.
    ///
    /// Callers must decide visibility through the version authority first;
    /// row stamps exist only for collection. Returns `None` only when the
    /// edge has no row mapping.
    pub fn get_projected_physical_by_edge_id(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Option<Vec<(String, Option<Value>)>> {
        if self.inline {
            return None;
        }
        let pos = self.mapped_row(edge_id)?;
        if pos >= self.visibility.len() {
            return None;
        }
        Some(
            self.resolve_projection(projection)
                .into_iter()
                .map(|(i, name)| {
                    let v = self.property_columns[i].get_at_ts(pos, query_ts);
                    (name.to_string(), v)
                })
                .collect(),
        )
    }

    /// Physical property projection for many edges without visibility filtering.
    ///
    /// Batch form of [`Self::get_projected_physical_by_edge_id`]: the
    /// projection resolves to column indices once and every edge reuses the
    /// mapping, so a projected adjacency pays one hash-based schema pass
    /// instead of one string scan per edge. Output order follows the input;
    /// each entry carries the same contract as the single-edge call (`None`
    /// for inline tables, unmapped edges and out-of-range rows).
    pub fn get_projected_physical_batch_by_edge_ids(
        &self,
        edge_ids: &[EdgeId],
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> ProjectedBatch {
        if self.inline {
            return edge_ids.iter().map(|_| None).collect();
        }
        let columns = self.resolve_projection(projection);
        edge_ids
            .iter()
            .map(|edge_id| {
                let pos = self.mapped_row(*edge_id)?;
                if pos >= self.visibility.len() {
                    return None;
                }
                Some(
                    columns
                        .iter()
                        .map(|(i, name)| {
                            let v = self.property_columns[*i].get_at_ts(pos, query_ts);
                            (name.to_string(), v)
                        })
                        .collect(),
                )
            })
            .collect()
    }
    pub fn read_properties_by_edge_id(&self, edge_id: EdgeId) -> Option<Vec<(String, Value)>> {
        if self.inline {
            return None;
        }
        let pos = self.mapped_row(edge_id)?;
        let result: Vec<(String, Value)> = self
            .property_schema
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let v = self.property_columns[i].get(pos)?;
                Some((s.name.clone(), v))
            })
            .collect();
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    /// Column index for one property name.
    fn column_index(&self, name: &str) -> Option<usize> {
        self.column_index.get(name).copied()
    }

    /// Snapshot value of one column cell for pushdown filtering.
    ///
    /// Reads through the version chain at `query_ts`; a `None` return means
    /// null at that snapshot, expressed through the column null bitmap rather
    /// than a materialized record.
    fn pushdown_cell(&self, row: usize, column: usize, query_ts: Timestamp) -> Option<Value> {
        self.property_columns.get(column)?.get_at_ts(row, query_ts)
    }

    /// Whether one edge matches every pushed predicate at `query_ts`.
    ///
    /// Column-scan layer: only predicate columns are read, each through its
    /// null bitmap, and no intermediate record is materialized. A missing
    /// column, a missing row mapping or a null cell never matches, mirroring
    /// the query NULL semantics where comparisons against NULL are false.
    pub fn matches_predicates_for_edge(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        predicates: &[crate::cursor::ScanPredicate],
    ) -> bool {
        if predicates.is_empty() {
            return true;
        }
        let Some(resolved) = self.resolve_predicate_columns(predicates) else {
            return false;
        };
        let Some(row) = self.mapped_row(edge_id) else {
            return false;
        };
        resolved.iter().all(|(column, predicate)| {
            self.pushdown_cell(row, *column, query_ts)
                .is_some_and(|value| predicate.matches_value(&value))
        })
    }

    /// Resolve predicate columns to positions once.
    ///
    /// Shared by the single-edge and batch matchers so a multi-edge filter
    /// pays one schema lookup per predicate instead of one per edge.
    /// `None` when any predicate references a missing column: the
    /// conjunction can never match, and callers report no hit.
    fn resolve_predicate_columns<'a>(
        &self,
        predicates: &'a [ScanPredicate],
    ) -> Option<Vec<(usize, &'a ScanPredicate)>> {
        predicates
            .iter()
            .map(|predicate| {
                self.column_index(predicate.column())
                    .map(|col| (col, predicate))
            })
            .collect()
    }

    /// Row-chunk liveness for one merged range set.
    ///
    /// One entry per zone-map chunk (`ZONE_MAP_CHUNK_ROWS` rows): a chunk is
    /// dead only when some range provably excludes its recorded bounds.
    /// Chunks without recorded bounds (all-null or never-written) stay live,
    /// and rows past the computed chunks read as live, so the pre-filter
    /// only ever skips rows that cannot match at any snapshot timestamp:
    /// zone bounds widen monotonically and contain every non-null value any
    /// snapshot can still observe through a version chain.
    fn live_row_chunks(&self, ranges: &[(usize, PredicateRange)]) -> Vec<bool> {
        let chunks = ranges
            .iter()
            .filter_map(|(col, _)| self.property_columns.get(*col))
            .map(|col| col.zone_maps().len())
            .max()
            .unwrap_or(0);
        let mut live = vec![true; chunks];
        for (chunk, slot) in live.iter_mut().enumerate() {
            for (col, range) in ranges {
                let Some(bounds) = self
                    .property_columns
                    .get(*col)
                    .and_then(|c| c.zone_for_chunk(chunk))
                else {
                    continue;
                };
                let (Some(min), Some(max)) = (&bounds.min, &bounds.max) else {
                    continue;
                };
                if !range.overlaps(min, max) {
                    *slot = false;
                    break;
                }
            }
        }
        live
    }

    /// Whether one row chunk may still match: chunks past the computed
    /// liveness map hold no recorded bounds and always scan.
    fn chunk_is_live(live: &[bool], row: usize) -> bool {
        live.get(row / ZONE_MAP_CHUNK_ROWS).copied().unwrap_or(true)
    }

    /// Filter edge ids by pushed predicates at the column-scan layer.
    ///
    /// Attribute equality and range predicates filter row numbers first;
    /// callers look up topology only for the returned hits. Nulls use bitmap
    /// semantics throughout and no intermediate records are materialized.
    /// `candidates` bounds the scan when the caller already holds row
    /// numbers; `None` scans every mapped edge.
    ///
    /// Two pre-filters run before the per-row version-chain reads: the
    /// merged column bounds short-circuit the whole scan, then the per-chunk
    /// zone bounds skip dead 1024-row chunks, so a selective predicate over
    /// a clustered column only decodes its surviving chunks. Output order
    /// follows the input in both arms: skipping never reorders survivors.
    pub fn filter_edge_ids_by_predicates(
        &self,
        predicates: &[crate::cursor::ScanPredicate],
        query_ts: Timestamp,
        candidates: Option<&[EdgeId]>,
    ) -> Vec<EdgeId> {
        if predicates.is_empty() {
            return candidates.map_or_else(|| self.edge_ids().collect(), <[EdgeId]>::to_vec);
        }
        let Some(resolved) = self.resolve_predicate_columns(predicates) else {
            return Vec::new();
        };
        // Zone-map short-circuit: every merged range must overlap its
        // column bounds, else no row can match. Bounds only widen and row
        // matching uses the same ordering, so a disjoint range provably
        // matches nothing at any snapshot timestamp. Columns without
        // recorded bounds (all-null columns included) are skipped: their
        // rows still go through the cell filter below.
        let merged = ScanPredicate::merged_ranges(predicates);
        for range in &merged {
            if let Some((min, max)) = self.prune_bounds(&range.column) {
                if !range.overlaps(&min, &max) {
                    return Vec::new();
                }
            }
        }
        // Per-chunk liveness from the same merged ranges: every predicate
        // column resolves here (resolution above already proved all
        // predicate columns exist), so a missing entry is unreachable and
        // fails closed to no match.
        let mut ranges = Vec::with_capacity(merged.len());
        for range in merged {
            let Some(col) = self.column_index(&range.column) else {
                return Vec::new();
            };
            ranges.push((col, range));
        }
        let live = self.live_row_chunks(&ranges);
        match candidates {
            Some(ids) => ids
                .iter()
                .copied()
                .filter(|edge_id| {
                    let Some(row) = self.mapped_row(*edge_id) else {
                        return false;
                    };
                    if !Self::chunk_is_live(&live, row) {
                        return false;
                    }
                    resolved.iter().all(|(column, predicate)| {
                        self.pushdown_cell(row, *column, query_ts)
                            .is_some_and(|value| predicate.matches_value(&value))
                    })
                })
                .collect(),
            None => self
                .edge_mappings()
                .filter_map(|(edge_id, row)| {
                    let row = row as usize;
                    if !Self::chunk_is_live(&live, row) {
                        return None;
                    }
                    resolved
                        .iter()
                        .all(|(column, predicate)| {
                            self.pushdown_cell(row, *column, query_ts)
                                .is_some_and(|value| predicate.matches_value(&value))
                        })
                        .then_some(edge_id)
                })
                .collect(),
        }
    }

    /// Typed column-major batch decode without visibility filtering.
    ///
    /// Native edge columnar entry: the caller holds one authority verdict per
    /// edge, so this decodes straight from the property columns into typed
    /// [`crate::cursor::ColumnValues`]. Projection resolves once; each column
    /// decodes according to its declared type and degrades to `General` on
    /// type mismatch, so one mixed column never forces the whole batch back
    /// to row transpose. Output order follows the input edge order.
    pub fn get_typed_columns_batch_by_edge_ids(
        &self,
        edge_ids: &[EdgeId],
        query_ts: Timestamp,
        projection: Option<&[String]>,
    ) -> Vec<(String, crate::cursor::ColumnValues)> {
        use crate::cursor::ColumnValues;
        use graphdb_core::types::DataType;
        if self.inline {
            return Vec::new();
        }
        let columns = self.resolve_projection(projection);
        let mut out = Vec::with_capacity(columns.len());
        for (col_idx, name) in columns {
            let data_type = self
                .property_schema
                .get(col_idx)
                .map(|s| s.data_type.clone())
                .unwrap_or(DataType::Empty);
            let Some(column) = self.property_columns.get(col_idx) else {
                out.push((
                    name.to_string(),
                    ColumnValues::General(vec![None; edge_ids.len()]),
                ));
                continue;
            };
            let typed = match data_type {
                DataType::BigInt => {
                    let mut values = Vec::with_capacity(edge_ids.len());
                    let mut valid = vec![0u8; edge_ids.len()];
                    let mut mixed = false;
                    for (i, edge_id) in edge_ids.iter().enumerate() {
                        let cell = self
                            .mapped_row(*edge_id)
                            .and_then(|pos| column.get_at_ts(pos, query_ts));
                        match cell {
                            Some(Value::BigInt(v)) => {
                                values.push(v);
                                valid[i] = 1;
                            }
                            None => values.push(0),
                            Some(_) => {
                                mixed = true;
                                break;
                            }
                        }
                    }
                    if mixed {
                        None
                    } else {
                        Some(ColumnValues::I64 { values, valid })
                    }
                }
                DataType::Double => {
                    let mut values = Vec::with_capacity(edge_ids.len());
                    let mut valid = vec![0u8; edge_ids.len()];
                    let mut mixed = false;
                    for (i, edge_id) in edge_ids.iter().enumerate() {
                        let cell = self
                            .mapped_row(*edge_id)
                            .and_then(|pos| column.get_at_ts(pos, query_ts));
                        match cell {
                            Some(Value::Double(v)) => {
                                values.push(v);
                                valid[i] = 1;
                            }
                            None => values.push(0.0),
                            Some(_) => {
                                mixed = true;
                                break;
                            }
                        }
                    }
                    if mixed {
                        None
                    } else {
                        Some(ColumnValues::F64 { values, valid })
                    }
                }
                DataType::Int => {
                    let mut values = Vec::with_capacity(edge_ids.len());
                    let mut valid = vec![0u8; edge_ids.len()];
                    let mut mixed = false;
                    for (i, edge_id) in edge_ids.iter().enumerate() {
                        let cell = self
                            .mapped_row(*edge_id)
                            .and_then(|pos| column.get_at_ts(pos, query_ts));
                        match cell {
                            Some(Value::Int(v)) => {
                                values.push(v);
                                valid[i] = 1;
                            }
                            None => values.push(0),
                            Some(_) => {
                                mixed = true;
                                break;
                            }
                        }
                    }
                    if mixed {
                        None
                    } else {
                        Some(ColumnValues::I32 { values, valid })
                    }
                }
                DataType::Bool => {
                    let mut values = Vec::with_capacity(edge_ids.len());
                    let mut valid = vec![0u8; edge_ids.len()];
                    let mut mixed = false;
                    for (i, edge_id) in edge_ids.iter().enumerate() {
                        let cell = self
                            .mapped_row(*edge_id)
                            .and_then(|pos| column.get_at_ts(pos, query_ts));
                        match cell {
                            Some(Value::Bool(v)) => {
                                values.push(u8::from(v));
                                valid[i] = 1;
                            }
                            None => values.push(0),
                            Some(_) => {
                                mixed = true;
                                break;
                            }
                        }
                    }
                    if mixed {
                        None
                    } else {
                        Some(ColumnValues::Bool { values, valid })
                    }
                }
                DataType::SmallInt => {
                    let mut values = Vec::with_capacity(edge_ids.len());
                    let mut valid = vec![0u8; edge_ids.len()];
                    let mut mixed = false;
                    for (i, edge_id) in edge_ids.iter().enumerate() {
                        let cell = self
                            .mapped_row(*edge_id)
                            .and_then(|pos| column.get_at_ts(pos, query_ts));
                        match cell {
                            Some(Value::SmallInt(v)) => {
                                values.push(v);
                                valid[i] = 1;
                            }
                            None => values.push(0),
                            Some(_) => {
                                mixed = true;
                                break;
                            }
                        }
                    }
                    if mixed {
                        None
                    } else {
                        Some(ColumnValues::I16 { values, valid })
                    }
                }
                DataType::Float => {
                    let mut values = Vec::with_capacity(edge_ids.len());
                    let mut valid = vec![0u8; edge_ids.len()];
                    let mut mixed = false;
                    for (i, edge_id) in edge_ids.iter().enumerate() {
                        let cell = self
                            .mapped_row(*edge_id)
                            .and_then(|pos| column.get_at_ts(pos, query_ts));
                        match cell {
                            Some(Value::Float(v)) => {
                                values.push(v);
                                valid[i] = 1;
                            }
                            None => values.push(0.0),
                            Some(_) => {
                                mixed = true;
                                break;
                            }
                        }
                    }
                    if mixed {
                        None
                    } else {
                        Some(ColumnValues::F32 { values, valid })
                    }
                }
                _ => None,
            };
            match typed {
                Some(values) => out.push((name.to_string(), values)),
                None => {
                    let general: Vec<Option<Value>> = edge_ids
                        .iter()
                        .map(|edge_id| {
                            self.mapped_row(*edge_id)
                                .and_then(|pos| column.get_at_ts(pos, query_ts))
                        })
                        .collect();
                    out.push((name.to_string(), ColumnValues::General(general)));
                }
            }
        }
        out
    }
}
