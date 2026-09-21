use super::CsrWithProperties;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::Value;

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
        match projection {
            None => Some(
                self.property_schema
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let v = self.property_columns[i].get_at_ts(pos, query_ts);
                        (s.name.clone(), v)
                    })
                    .collect(),
            ),
            Some(names) => {
                if names.is_empty() {
                    return Some(Vec::new());
                }
                Some(
                    self.property_schema
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| names.iter().any(|n| n == &s.name))
                        .map(|(i, s)| {
                            let v = self.property_columns[i].get_at_ts(pos, query_ts);
                            (s.name.clone(), v)
                        })
                        .collect(),
                )
            }
        }
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
        match projection {
            None => Some(
                self.property_schema
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let v = self.property_columns[i].get_at_ts(pos, query_ts);
                        (s.name.clone(), v)
                    })
                    .collect(),
            ),
            Some(names) => {
                if names.is_empty() {
                    return Some(Vec::new());
                }
                Some(
                    self.property_schema
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| names.iter().any(|n| n == &s.name))
                        .map(|(i, s)| {
                            let v = self.property_columns[i].get_at_ts(pos, query_ts);
                            (s.name.clone(), v)
                        })
                        .collect(),
                )
            }
        }
    }

    /// Read non-nullable properties for an edge by its EdgeId (no MVCC filtering).
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
        let Some(row) = self.mapped_row(edge_id) else {
            return false;
        };
        for predicate in predicates {
            let Some(column) = self.column_index(predicate.column()) else {
                return false;
            };
            let Some(value) = self.pushdown_cell(row, column, query_ts) else {
                return false;
            };
            if !predicate.matches_value(&value) {
                return false;
            }
        }
        true
    }

    /// Filter edge ids by pushed predicates at the column-scan layer.
    ///
    /// Attribute equality and range predicates filter row numbers first;
    /// callers look up topology only for the returned hits. Nulls use bitmap
    /// semantics throughout and no intermediate records are materialized.
    /// `candidates` bounds the scan when the caller already holds row
    /// numbers; `None` scans every mapped edge.
    pub fn filter_edge_ids_by_predicates(
        &self,
        predicates: &[crate::cursor::ScanPredicate],
        query_ts: Timestamp,
        candidates: Option<&[EdgeId]>,
    ) -> Vec<EdgeId> {
        if predicates.is_empty() {
            return candidates.map_or_else(|| self.edge_ids().collect(), <[EdgeId]>::to_vec);
        }
        let resolved: Vec<(usize, &crate::cursor::ScanPredicate)> = predicates
            .iter()
            .map(|predicate| (self.column_index(predicate.column()), predicate))
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|(index, predicate)| index.map(|column| (column, predicate)))
            .collect();
        if resolved.len() != predicates.len() {
            return Vec::new();
        }
        match candidates {
            Some(ids) => ids
                .iter()
                .copied()
                .filter(|edge_id| {
                    let Some(row) = self.mapped_row(*edge_id) else {
                        return false;
                    };
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
}
