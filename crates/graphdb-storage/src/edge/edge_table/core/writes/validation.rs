use super::super::EdgeStore;
use crate::edge::edge_table::staging::EdgeStagingBatch;
use crate::edge::EdgeStrategy;
use graphdb_core::{StorageError, StorageResult, Value};

impl EdgeStore {
    /// Convert staged property values to column positions with cast values.
    ///
    /// Each name resolves once through the column index cache; the returned
    /// positions feed the property store directly, so the per-edge insert
    /// performs no further name lookup, string clone or string comparison.
    /// Names for the secondary property index resolve from the schema by
    /// the same positions at the call site.
    pub(crate) fn convert_property_values(
        &self,
        property_values: &[(String, Value)],
    ) -> StorageResult<Vec<(usize, Value)>> {
        let mut converted_values: Vec<(usize, Value)> = Vec::with_capacity(property_values.len());
        for (name, value) in property_values {
            let prop_idx = self
                .property_index_cache
                .get(name)
                .copied()
                .ok_or_else(|| StorageError::column_not_found(name.clone()))?;
            let prop_def = &self.schema.properties[prop_idx];

            if value.data_type() != prop_def.data_type {
                let converted = value.try_cast_to(&prop_def.data_type)?;
                converted_values.push((prop_idx, converted));
            } else {
                converted_values.push((prop_idx, value.clone()));
            }
        }
        Ok(converted_values)
    }

    pub(super) fn prevalidate_staging_batch(&self, batch: &EdgeStagingBatch) -> StorageResult<()> {
        // Insert-only batches skip the order-sensitive cancel bookkeeping:
        // without deletes no insert can cancel, so sorted duplicate scans
        // replace the per-batch hash sets.
        if batch.staged_deletes().is_empty() && !batch.staged_inserts().is_empty() {
            return self.prevalidate_inserts_sorted(batch);
        }
        use std::collections::HashSet;
        let mut seen_inserts: HashSet<(u32, u32, i64)> = HashSet::new();
        let mut seen_deletes: HashSet<(u32, u32, i64)> = HashSet::new();
        let mut seen_single_src: HashSet<u32> = HashSet::new();
        let mut seen_single_dst: HashSet<u32> = HashSet::new();
        let single_out = self.schema.oe_strategy == EdgeStrategy::Single;
        let single_in = self.schema.ie_strategy == EdgeStrategy::Single;
        for ord in batch.ordered() {
            if ord.is_insert {
                let ins = &batch.staged_inserts()[ord.slot];
                for (name, _) in &ins.properties {
                    if !self.property_index_cache.contains_key(name) {
                        return Err(StorageError::column_not_found(name.clone()));
                    }
                }
                let key = (ins.src, ins.dst, ins.rank);
                if seen_inserts.contains(&key) {
                    return Err(StorageError::edge_already_exists(format!(
                        "{} -> {}@{}",
                        ins.src, ins.dst, ins.rank
                    )));
                }
                if single_out && seen_single_src.contains(&ins.src) {
                    return Err(StorageError::conflict(format!(
                        "Single out-edge strategy already holds a live edge for src={}",
                        ins.src
                    )));
                }
                if single_in && seen_single_dst.contains(&ins.dst) {
                    return Err(StorageError::conflict(format!(
                        "Single in-edge strategy already holds a live edge for dst={}",
                        ins.dst
                    )));
                }
                if single_out {
                    let mut uncovered = false;
                    let mut any_live = false;
                    self.out_csr.visit_physical(ins.src, |nbr| {
                        if self.is_visible(nbr.edge_id, ins.create_ts) {
                            any_live = true;
                            if !seen_deletes.contains(&(ins.src, nbr.endpoint, nbr.rank)) {
                                uncovered = true;
                                return false;
                            }
                        }
                        true
                    });
                    if any_live && uncovered {
                        return Err(StorageError::conflict(format!(
                            "Single out-edge strategy already holds a live edge for src={}",
                            ins.src
                        )));
                    }
                }
                if single_in {
                    let mut uncovered = false;
                    let mut any_live = false;
                    self.in_csr.visit_physical(ins.dst, |nbr| {
                        if self.is_visible(nbr.edge_id, ins.create_ts) {
                            any_live = true;
                            if !seen_deletes.contains(&(nbr.endpoint, ins.dst, nbr.rank)) {
                                uncovered = true;
                                return false;
                            }
                        }
                        true
                    });
                    if any_live && uncovered {
                        return Err(StorageError::conflict(format!(
                            "Single in-edge strategy already holds a live edge for dst={}",
                            ins.dst
                        )));
                    }
                }
                if !seen_deletes.contains(&key)
                    && self.has_edge(ins.src, ins.dst, ins.rank, ins.create_ts)
                {
                    return Err(StorageError::edge_already_exists(format!(
                        "{} -> {}@{}",
                        ins.src, ins.dst, ins.rank
                    )));
                }
                if seen_deletes.contains(&key) {
                    seen_deletes.remove(&key);
                }
                seen_inserts.insert(key);
                if single_out {
                    seen_single_src.insert(ins.src);
                }
                if single_in {
                    seen_single_dst.insert(ins.dst);
                }
            } else {
                let del = &batch.staged_deletes()[ord.slot];
                let key = (del.src, del.dst, del.rank);
                if seen_inserts.remove(&key) {
                    if single_out && !seen_inserts.iter().any(|(s, _, _)| *s == del.src) {
                        seen_single_src.remove(&del.src);
                    }
                    if single_in && !seen_inserts.iter().any(|(_, d, _)| *d == del.dst) {
                        seen_single_dst.remove(&del.dst);
                    }
                } else {
                    seen_deletes.insert(key);
                }
            }
        }
        Ok(())
    }

    /// Validate an insert-only staging batch without per-batch hash sets.
    ///
    /// With no deletes in the batch, inserts cannot cancel each other, so
    /// intra-batch duplicates and Single-slot conflicts reduce to sorted
    /// adjacency checks. Existence and occupancy checks against committed
    /// state are unchanged from the general path.
    pub(super) fn prevalidate_inserts_sorted(&self, batch: &EdgeStagingBatch) -> StorageResult<()> {
        let inserts = batch.staged_inserts();
        for ins in inserts {
            for (name, _) in &ins.properties {
                if !self.property_index_cache.contains_key(name) {
                    return Err(StorageError::column_not_found(name.clone()));
                }
            }
        }
        let mut by_key: Vec<(u32, u32, i64)> = inserts
            .iter()
            .map(|ins| (ins.src, ins.dst, ins.rank))
            .collect();
        by_key.sort_unstable();
        for window in by_key.windows(2) {
            if window[0] == window[1] {
                return Err(StorageError::edge_already_exists(format!(
                    "{} -> {}@{}",
                    window[1].0, window[1].1, window[1].2
                )));
            }
        }
        let single_out = self.schema.oe_strategy == EdgeStrategy::Single;
        let single_in = self.schema.ie_strategy == EdgeStrategy::Single;
        if single_out {
            let mut by_src: Vec<u32> = inserts.iter().map(|ins| ins.src).collect();
            by_src.sort_unstable();
            for window in by_src.windows(2) {
                if window[0] == window[1] {
                    return Err(StorageError::conflict(format!(
                        "Single out-edge strategy already holds a live edge for src={}",
                        window[1]
                    )));
                }
            }
        }
        if single_in {
            let mut by_dst: Vec<u32> = inserts.iter().map(|ins| ins.dst).collect();
            by_dst.sort_unstable();
            for window in by_dst.windows(2) {
                if window[0] == window[1] {
                    return Err(StorageError::conflict(format!(
                        "Single in-edge strategy already holds a live edge for dst={}",
                        window[1]
                    )));
                }
            }
        }
        for ins in inserts {
            if single_out {
                let mut occupied = false;
                self.out_csr.visit_physical(ins.src, |nbr| {
                    if self.is_visible(nbr.edge_id, ins.create_ts) {
                        occupied = true;
                        return false;
                    }
                    true
                });
                if occupied {
                    return Err(StorageError::conflict(format!(
                        "Single out-edge strategy already holds a live edge for src={}",
                        ins.src
                    )));
                }
            }
            if single_in {
                let mut occupied = false;
                self.in_csr.visit_physical(ins.dst, |nbr| {
                    if self.is_visible(nbr.edge_id, ins.create_ts) {
                        occupied = true;
                        return false;
                    }
                    true
                });
                if occupied {
                    return Err(StorageError::conflict(format!(
                        "Single in-edge strategy already holds a live edge for dst={}",
                        ins.dst
                    )));
                }
            }
            if self.has_edge(ins.src, ins.dst, ins.rank, ins.create_ts) {
                return Err(StorageError::edge_already_exists(format!(
                    "{} -> {}@{}",
                    ins.src, ins.dst, ins.rank
                )));
            }
        }
        Ok(())
    }
}
