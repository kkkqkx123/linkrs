use super::super::EdgeStore;
use crate::edge::edge_table::staging::EdgeStagingBatch;
use crate::edge::BatchInsertEntry;
use crate::edge::{CsrBase, MutableCsrTrait};
use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::{StorageError, StorageResult, Value};

impl EdgeStore {
    pub fn insert_edge(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        property_values: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if !self.schema.has_out() && !self.schema.has_in() {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: table stores neither direction".to_string(),
            ));
        }

        // Single-entry staging commit: the batch owns the whole multi-step
        // write, so failure handling lives in one place below instead of in
        // per-step compensation branches here.
        let mut batch = EdgeStagingBatch::new();
        batch.stage_insert(src, dst, rank, property_values, ts);
        self.commit_staging_batch(batch).map(|_| ())
    }

    /// Commit many inserts of one edge type in a single staging batch.
    ///
    /// One prevalidation, one topology reservation pass and one live-index
    /// rebuild for the whole batch instead of one per edge. Entries apply in
    /// slice order with the same per-entry effects as repeated `insert_edge`
    /// calls, including out/in symmetry and rollback of the applied prefix on
    /// failure. An empty table takes the grouped direct-write path instead:
    /// no rows, no timestamps and no frozen groups exist yet, so one log
    /// append plus one reservation pass per direction produces exactly what
    /// the per-entry staging commit would. Empty bundled tables qualify only
    /// when every rank is pinned to zero (then each `(src, dst)` key is
    /// unique and the value pass addresses one slot per leg); anything else
    /// stays on the staging path so inline values ride the value column.
    pub fn insert_edges_batch(&mut self, entries: &[BatchInsertEntry]) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if !self.schema.has_out() && !self.schema.has_in() {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: table stores neither direction".to_string(),
            ));
        }

        if self.is_empty_for_bulk_import() && self.bulk_import_rank_pinned(entries) {
            self.bulk_import_edges(entries)?;
            return Ok(());
        }
        let mut batch = EdgeStagingBatch::new();
        for (src, dst, rank, property_values, ts) in entries {
            batch.stage_insert(*src, *dst, *rank, property_values, *ts);
        }
        self.commit_staging_batch(batch).map(|_| ())
    }

    /// Whether the table is empty for a direct partitioned bulk import.
    fn is_empty_for_bulk_import(&self) -> bool {
        self.next_edge_id == EdgeId(0)
            && self.out_csr.edge_count() == 0
            && self.in_csr.edge_count() == 0
    }

    /// Whether a batch may take the bulk path on a bundled table.
    ///
    /// Non-bundled tables always qualify; bundled tables qualify only when
    /// every rank is pinned to zero, so each `(src, dst)` key is unique
    /// after prevalidation and the inline-value pass addresses exactly one
    /// slot per leg by endpoint.
    fn bulk_import_rank_pinned(&self, entries: &[BatchInsertEntry]) -> bool {
        if !self.is_bundled() {
            return true;
        }
        entries.iter().all(|(_, _, rank, _, _)| *rank == 0)
    }

    /// Partitioned direct-write bulk import for initial loads.
    ///
    /// Empty-table fast path mirroring the partitioned direct-pack idea:
    /// one prevalidation, one write-ahead append, one reservation pass per
    /// direction, then one grouped topology write per direction preserving
    /// per-edge timestamps, followed by batched authority, property, owner
    /// and index writes. Topology uses the grouped bulk path (single
    /// reservation and single live-set rebuild per row) instead of the
    /// per-entry insert loop, so wide fanouts avoid per-edge index
    /// maintenance during the topology phase. The commit point is unchanged:
    /// groups stay dirty with their append sidecars and the next checkpoint
    /// persists through the existing manifest protocol. Non-empty tables
    /// must use `insert_edges_batch`. Bundled tables take the
    /// bundled-specific import below instead of the columnar property
    /// writes.
    pub fn bulk_import_edges(&mut self, entries: &[BatchInsertEntry]) -> StorageResult<usize> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        if self.migration_pending_checkpoint {
            return Err(StorageError::invalid_operation(
                "table requires a checkpoint after record-form switch before further writes"
                    .to_string(),
            ));
        }
        if !self.schema.has_out() && !self.schema.has_in() {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: table stores neither direction".to_string(),
            ));
        }
        if self.is_bundled() {
            return self.bulk_import_bundled_edges(entries);
        }
        if entries.is_empty() {
            return Ok(0);
        }
        if !self.is_empty_for_bulk_import() {
            return Err(StorageError::invalid_operation(
                "bulk import requires an empty table: use insert_edges_batch for incremental writes"
                    .to_string(),
            ));
        }
        let mut batch = EdgeStagingBatch::new();
        for (src, dst, rank, property_values, ts) in entries {
            batch.stage_insert(*src, *dst, *rank, property_values, *ts);
        }
        self.prevalidate_staging_batch(&batch)?;
        let mut converted: Vec<Vec<(usize, Value)>> = Vec::with_capacity(entries.len());
        for (_, _, _, property_values, _) in entries {
            converted.push(self.convert_property_values(property_values)?);
        }
        if let Some(dir) = self.wal_dir.clone() {
            let ops: Vec<crate::edge::edge_table::wal::EdgeWalOp> = entries
                .iter()
                .map(|(src, dst, rank, property_values, ts)| {
                    crate::edge::edge_table::wal::EdgeWalOp::Insert {
                        src: *src,
                        dst: *dst,
                        rank: *rank,
                        properties: property_values.to_vec(),
                        create_ts: *ts,
                    }
                })
                .collect();
            crate::edge::edge_table::wal::append_ops(&dir, &ops)?;
        }
        let mut by_src: Vec<u32> = entries.iter().map(|(src, _, _, _, _)| *src).collect();
        by_src.sort_unstable();
        let mut out_counts: Vec<(u32, usize)> = Vec::new();
        for src in by_src {
            if let Some(last) = out_counts.last_mut() {
                if last.0 == src {
                    last.1 += 1;
                    continue;
                }
            }
            out_counts.push((src, 1));
        }
        let mut by_dst: Vec<u32> = entries.iter().map(|(_, dst, _, _, _)| *dst).collect();
        by_dst.sort_unstable();
        let mut in_counts: Vec<(u32, usize)> = Vec::new();
        for dst in by_dst {
            if let Some(last) = in_counts.last_mut() {
                if last.0 == dst {
                    last.1 += 1;
                    continue;
                }
            }
            in_counts.push((dst, 1));
        }
        if self.schema.has_out() {
            if let Err(e) = self.out_csr.reserve_for_batch(&out_counts) {
                log::debug!("bulk import reserve out skipped: {}", e);
            }
        }
        if self.schema.has_in() {
            if let Err(e) = self.in_csr.reserve_for_batch(&in_counts) {
                log::debug!("bulk import reserve in skipped: {}", e);
            }
        }
        let base = self.next_edge_id.0;
        let ids: Vec<EdgeId> = (0..entries.len())
            .map(|i| EdgeId(base + i as u64))
            .collect();
        self.next_edge_id = EdgeId(base + entries.len() as u64);
        let has_out = self.schema.has_out();
        let has_in = self.schema.has_in();
        let mut out_quads: Vec<(u32, VertexId, EdgeId, Timestamp)> =
            Vec::with_capacity(entries.len());
        let mut in_quads: Vec<(u32, VertexId, EdgeId, Timestamp)> =
            Vec::with_capacity(entries.len());
        for (i, (src, dst, rank, _, ts)) in entries.iter().enumerate() {
            let edge_id = ids[i];
            if has_out {
                out_quads.push((*src, Self::edge_endpoint_key(*dst, *rank), edge_id, *ts));
            }
            if has_in {
                in_quads.push((*dst, Self::edge_endpoint_key(*src, *rank), edge_id, *ts));
            }
        }
        if has_out && !out_quads.is_empty() {
            if let Err(e) = self.out_csr.batch_put_edges_with_ts(&out_quads, false) {
                for (src, _, edge_id, _) in &out_quads {
                    self.out_csr.rollback_insert(*src, *edge_id);
                }
                self.next_edge_id = EdgeId(base);
                return Err(e);
            }
        }
        if has_in && !in_quads.is_empty() {
            if let Err(e) = self.in_csr.batch_put_edges_with_ts(&in_quads, false) {
                for (src, _, edge_id, _) in &out_quads {
                    self.out_csr.rollback_insert(*src, *edge_id);
                }
                for (dst, _, edge_id, _) in &in_quads {
                    self.in_csr.rollback_insert(*dst, *edge_id);
                }
                self.next_edge_id = EdgeId(base);
                return Err(e);
            }
        }
        let inline = self.properties.is_inline_stub();
        let mut applied = 0usize;
        for (i, (src, dst, rank, _, ts)) in entries.iter().enumerate() {
            let edge_id = ids[i];
            self.mvcc.record_creation(edge_id, *ts);
            if !inline {
                if let Err(e) = self
                    .properties
                    .insert_for_edge_at(edge_id, &converted[i], *ts)
                {
                    for (j, (rsrc, rdst, rrank, _, rts)) in entries.iter().enumerate() {
                        self.erase_applied_insert(*rsrc, *rdst, *rrank, ids[j], *rts);
                    }
                    self.next_edge_id = EdgeId(base);
                    return Err(e);
                }
            }
            if self.property_index.is_some() && !inline {
                let label = self.label;
                let outcomes: Vec<(String, StorageResult<()>, u64)> =
                    if let Some(ref mut index) = self.property_index {
                        converted[i]
                            .iter()
                            .map(|(prop_idx, prop_value)| {
                                let started = std::time::Instant::now();
                                let prop_name = &self.schema.properties[*prop_idx].name;
                                let result = index
                                    .insert(prop_name, prop_value, *src, *dst, *rank, label, *ts);
                                let latency = started.elapsed().as_millis() as u64;
                                (prop_name.clone(), result, latency)
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                for (prop_name, result, latency) in outcomes {
                    if let Err(e) = self.note_index_result(&prop_name, result, latency) {
                        for (j, (rsrc, rdst, rrank, _, rts)) in entries.iter().enumerate() {
                            self.erase_applied_insert(*rsrc, *rdst, *rrank, ids[j], *rts);
                        }
                        self.next_edge_id = EdgeId(base);
                        return Err(e);
                    }
                }
            }
            self.edge_owner
                .insert(edge_id, self.owner_gid_for(*src, *dst));
            applied += 1;
        }
        self.mark_properties_dirty();
        for (src, dst, _, _, _) in entries {
            let owner = self.owner_gid_for(*src, *dst);
            *self.group_write_counts.entry(owner).or_insert(0) += 1;
        }
        for (_, _, _, property_values, _) in entries {
            self.observe_form_write(property_values);
        }
        if let Some(ts) = entries.iter().map(|(_, _, _, _, ts)| *ts).max() {
            let pressured = self.check_and_apply_write_backpressure(ts);
            self.maybe_run_auto_maintenance();
            if pressured {
                log::warn!(
                    "edge table '{}' over mutable CSR budget, synchronous maintenance ran",
                    self.label_name
                );
            }
        }
        for edge_id in &ids {
            self.ensure_copies_consistent(*edge_id)?;
        }
        Ok(applied)
    }

    /// Partitioned direct-write bulk import for empty bundled tables.
    ///
    /// Same commit point and ordering as [`Self::bulk_import_edges`],
    /// except the single scalar rides the CSR value column instead of the
    /// columnar store: topology goes through the grouped path (bundled
    /// groups take its per-edge fallback, inserting NULL slots with the
    /// same dirt and append-log entries as the single-edge path), then one
    /// value pass fills each slot by endpoint. Only rank-pinned batches
    /// qualify; anything else keeps the original rejection and must use
    /// `insert_edges_batch`. Rollback erases the whole batch through the
    /// bundled-aware erase path and rewinds the edge-id allocator, so a
    /// failed import leaves the table empty.
    fn bulk_import_bundled_edges(&mut self, entries: &[BatchInsertEntry]) -> StorageResult<usize> {
        if entries.is_empty() {
            return Ok(0);
        }
        if !self.is_empty_for_bulk_import() {
            return Err(StorageError::invalid_operation(
                "bulk import requires an empty table: use insert_edges_batch for incremental writes"
                    .to_string(),
            ));
        }
        if !self.bulk_import_rank_pinned(entries) {
            return Err(StorageError::invalid_operation(
                "bulk import rejects ranked batches on bundled tables: use insert_edges_batch so inline values ride the value column, or migrate to the columnar form first, see migration_plan/migrate_record_form/switch_record_form_online"
                    .to_string(),
            ));
        }
        let mut batch = EdgeStagingBatch::new();
        for (src, dst, rank, property_values, ts) in entries {
            batch.stage_insert(*src, *dst, *rank, property_values, *ts);
        }
        self.prevalidate_staging_batch(&batch)?;
        let mut inline_values: Vec<Option<u64>> = Vec::with_capacity(entries.len());
        for (_, _, _, property_values, _) in entries {
            inline_values.push(self.convert_bundled_value(property_values)?);
        }
        if let Some(dir) = self.wal_dir.clone() {
            let ops: Vec<crate::edge::edge_table::wal::EdgeWalOp> = entries
                .iter()
                .map(|(src, dst, rank, property_values, ts)| {
                    crate::edge::edge_table::wal::EdgeWalOp::Insert {
                        src: *src,
                        dst: *dst,
                        rank: *rank,
                        properties: property_values.to_vec(),
                        create_ts: *ts,
                    }
                })
                .collect();
            crate::edge::edge_table::wal::append_ops(&dir, &ops)?;
        }
        let mut by_src: Vec<u32> = entries.iter().map(|(src, _, _, _, _)| *src).collect();
        by_src.sort_unstable();
        let mut out_counts: Vec<(u32, usize)> = Vec::new();
        for src in by_src {
            if let Some(last) = out_counts.last_mut() {
                if last.0 == src {
                    last.1 += 1;
                    continue;
                }
            }
            out_counts.push((src, 1));
        }
        let mut by_dst: Vec<u32> = entries.iter().map(|(_, dst, _, _, _)| *dst).collect();
        by_dst.sort_unstable();
        let mut in_counts: Vec<(u32, usize)> = Vec::new();
        for dst in by_dst {
            if let Some(last) = in_counts.last_mut() {
                if last.0 == dst {
                    last.1 += 1;
                    continue;
                }
            }
            in_counts.push((dst, 1));
        }
        if self.schema.has_out() {
            if let Err(e) = self.out_csr.reserve_for_batch(&out_counts) {
                log::debug!("bulk import reserve out skipped: {}", e);
            }
        }
        if self.schema.has_in() {
            if let Err(e) = self.in_csr.reserve_for_batch(&in_counts) {
                log::debug!("bulk import reserve in skipped: {}", e);
            }
        }
        let base = self.next_edge_id.0;
        let ids: Vec<EdgeId> = (0..entries.len())
            .map(|i| EdgeId(base + i as u64))
            .collect();
        self.next_edge_id = EdgeId(base + entries.len() as u64);
        let has_out = self.schema.has_out();
        let has_in = self.schema.has_in();
        let mut out_quads: Vec<(u32, VertexId, EdgeId, Timestamp)> =
            Vec::with_capacity(entries.len());
        let mut in_quads: Vec<(u32, VertexId, EdgeId, Timestamp)> =
            Vec::with_capacity(entries.len());
        for (i, (src, dst, rank, _, ts)) in entries.iter().enumerate() {
            let edge_id = ids[i];
            if has_out {
                out_quads.push((*src, Self::edge_endpoint_key(*dst, *rank), edge_id, *ts));
            }
            if has_in {
                in_quads.push((*dst, Self::edge_endpoint_key(*src, *rank), edge_id, *ts));
            }
        }
        if has_out && !out_quads.is_empty() {
            if let Err(e) = self.out_csr.batch_put_edges_with_ts(&out_quads, false) {
                for (src, _, edge_id, _) in &out_quads {
                    self.out_csr.rollback_insert(*src, *edge_id);
                }
                self.next_edge_id = EdgeId(base);
                return Err(e);
            }
        }
        if has_in && !in_quads.is_empty() {
            if let Err(e) = self.in_csr.batch_put_edges_with_ts(&in_quads, false) {
                for (src, _, edge_id, _) in &out_quads {
                    self.out_csr.rollback_insert(*src, *edge_id);
                }
                for (dst, _, edge_id, _) in &in_quads {
                    self.in_csr.rollback_insert(*dst, *edge_id);
                }
                self.next_edge_id = EdgeId(base);
                return Err(e);
            }
        }
        // Every rank is zero and keys are prevalidated unique, so endpoint
        // addressing hits exactly one slot per leg.
        for (i, (src, dst, rank, _, ts)) in entries.iter().enumerate() {
            let edge_id = ids[i];
            let value = inline_values[i];
            let out_ok = !has_out
                || self
                    .out_csr
                    .bundled_set_value_by_endpoint(*src, *dst, value);
            let in_ok = !has_in || self.in_csr.bundled_set_value_by_endpoint(*dst, *src, value);
            if !out_ok || !in_ok {
                for (j, (rsrc, rdst, rrank, _, rts)) in entries.iter().enumerate() {
                    self.erase_applied_insert(*rsrc, *rdst, *rrank, ids[j], *rts);
                }
                self.next_edge_id = EdgeId(base);
                return Err(StorageError::data_corruption(format!(
                    "bundled bulk import lost the inline value slot for edge {:?}",
                    edge_id
                )));
            }
            self.mvcc.record_creation(edge_id, *ts);
            if self.property_index.is_some() {
                if let Some((prop_name, prop_value)) = self.bundled_index_pair(value) {
                    let label = self.label;
                    let (result, latency) = if let Some(ref mut index) = self.property_index {
                        let started = std::time::Instant::now();
                        let result =
                            index.insert(&prop_name, &prop_value, *src, *dst, *rank, label, *ts);
                        (result, started.elapsed().as_millis() as u64)
                    } else {
                        (Ok(()), 0)
                    };
                    if let Err(e) = self.note_index_result(&prop_name, result, latency) {
                        for (j, (rsrc, rdst, rrank, _, rts)) in entries.iter().enumerate() {
                            self.erase_applied_insert(*rsrc, *rdst, *rrank, ids[j], *rts);
                        }
                        self.next_edge_id = EdgeId(base);
                        return Err(e);
                    }
                }
            }
            self.edge_owner
                .insert(edge_id, self.owner_gid_for(*src, *dst));
        }
        self.mark_properties_dirty();
        for (src, dst, _, _, _) in entries {
            let owner = self.owner_gid_for(*src, *dst);
            *self.group_write_counts.entry(owner).or_insert(0) += 1;
        }
        for (_, _, _, property_values, _) in entries {
            self.observe_form_write(property_values);
        }
        if let Some(ts) = entries.iter().map(|(_, _, _, _, ts)| *ts).max() {
            let pressured = self.check_and_apply_write_backpressure(ts);
            self.maybe_run_auto_maintenance();
            if pressured {
                log::warn!(
                    "edge table '{}' over mutable CSR budget, synchronous maintenance ran",
                    self.label_name
                );
            }
        }
        for edge_id in &ids {
            self.ensure_copies_consistent(*edge_id)?;
        }
        Ok(ids.len())
    }

    /// Move one staged insert into the committed structures.
    ///
    /// Self-contained: a failure cleans up only this entry, so the batch
    /// rollback above only handles entries that fully applied.
    pub(super) fn apply_staged_insert(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        property_values: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<EdgeId> {
        if self.is_bundled() {
            return self.apply_staged_insert_bundled(src, dst, rank, property_values, ts);
        }
        let converted_values = self.convert_property_values(property_values)?;
        let edge_id = self.next_edge_id.fetch_add();

        // No existence re-check here by design. The batch prevalidation is
        // the single main existence check: it already rejected duplicate keys
        // and occupied Single slots for this batch. The topology insert
        // below is the light recheck: its live-set/slot guard rejects the
        // same conflicts in O(1) without another row scan, and the failure
        // path underneath cleans up the record staged above.

        self.mvcc.record_creation(edge_id, ts);

        if let Err(e) = self
            .properties
            .insert_for_edge_at(edge_id, &converted_values, ts)
        {
            self.mvcc.remove_edge_timestamps(edge_id);
            return Err(e);
        }

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);
        let has_out = self.schema.has_out();
        let has_in = self.schema.has_in();
        if has_out {
            if let Err(e) = self.out_csr.insert_edge(src, dst_key, edge_id, ts) {
                if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
                    self.properties.release_row(row);
                }
                self.mvcc.remove_edge_timestamps(edge_id);
                self.mark_properties_dirty();
                self.debug_assert_copies_consistent(edge_id);
                return Err(e);
            }
        }

        if has_in {
            if let Err(e) = self.in_csr.insert_edge(dst, src_key, edge_id, ts) {
                // Roll back the out-direction insertion physically so no
                // tombstone residue remains; fall back to logical deletion if
                // the entry cannot be located.
                if has_out && !self.out_csr.rollback_insert(src, edge_id) {
                    let _ = self.out_csr.delete_edge(src, edge_id, ts);
                }
                if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
                    self.properties.release_row(row);
                }
                let _ = self.properties.mark_deleted(edge_id, ts);
                self.mvcc.remove_edge_timestamps(edge_id);
                self.mark_properties_dirty();
                self.debug_assert_copies_consistent(edge_id);
                return Err(e);
            }
        }

        if self.property_index.is_some() {
            let label = self.label;
            let outcomes: Vec<(String, StorageResult<()>, u64)> = if let Some(ref mut index) =
                self.property_index
            {
                converted_values
                    .iter()
                    .map(|(prop_idx, prop_value)| {
                        let started = std::time::Instant::now();
                        let prop_name = &self.schema.properties[*prop_idx].name;
                        let result = index.insert(prop_name, prop_value, src, dst, rank, label, ts);
                        let latency = started.elapsed().as_millis() as u64;
                        (prop_name.clone(), result, latency)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let strong = self.index_consistency == crate::edge::IndexConsistency::Strong;
            for (prop_name, result, latency) in outcomes {
                let failed = result.is_err();
                if let Err(e) = self.note_index_result(&prop_name, result, latency) {
                    // Strong contract: roll back the just-applied edge so no
                    // primary residue outlives its index entry.
                    self.erase_applied_insert(src, dst, rank, edge_id, ts);
                    return Err(e);
                }
                if strong && failed {
                    self.erase_applied_insert(src, dst, rank, edge_id, ts);
                    return Err(StorageError::invalid_operation(format!(
                        "strong index write failed for '{}'",
                        prop_name
                    )));
                }
            }
        }

        self.mark_properties_dirty();
        self.edge_owner
            .insert(edge_id, self.owner_gid_for(src, dst));
        self.debug_assert_copies_consistent(edge_id);
        self.observe_form_write(property_values);
        Ok(edge_id)
    }
}
