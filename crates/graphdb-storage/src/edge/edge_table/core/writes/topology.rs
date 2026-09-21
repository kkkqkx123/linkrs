use super::super::EdgeStore;
use crate::edge::edge_table::staging::EdgeStagingBatch;
use crate::edge::BatchInsertEntry;
use crate::edge::EdgeStrategy;
use crate::edge::MutableCsrTrait;
use graphdb_core::types::{EdgeId, Timestamp};
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

        if self.schema.oe_strategy == EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: out-edge strategy is None".to_string(),
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
    /// failure.
    pub fn insert_edges_batch(&mut self, entries: &[BatchInsertEntry]) -> StorageResult<()> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }

        if self.schema.oe_strategy == EdgeStrategy::None {
            return Err(StorageError::invalid_operation(
                "Cannot insert edge: out-edge strategy is None".to_string(),
            ));
        }

        let mut batch = EdgeStagingBatch::new();
        for (src, dst, rank, property_values, ts) in entries {
            batch.stage_insert(*src, *dst, *rank, property_values, *ts);
        }
        self.commit_staging_batch(batch).map(|_| ())
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
        if let Err(e) = self.out_csr.insert_edge(src, dst_key, edge_id, ts) {
            if let Some(row) = self.properties.remove_edge_mapping(edge_id) {
                self.properties.release_row(row);
            }
            self.mvcc.remove_edge_timestamps(edge_id);
            self.mark_properties_dirty();
            self.debug_assert_copies_consistent(edge_id);
            return Err(e);
        }

        if let Err(e) = self.in_csr.insert_edge(dst, src_key, edge_id, ts) {
            // Roll back the out-direction insertion physically so no
            // tombstone residue remains; fall back to logical deletion if
            // the entry cannot be located.
            if !self.out_csr.rollback_insert(src, edge_id) {
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
            for (prop_name, result, latency) in outcomes {
                self.note_index_result(&prop_name, result, latency);
            }
        }

        self.mark_properties_dirty();
        self.edge_owner
            .insert(edge_id, self.owner_gid_for(src, dst));
        self.debug_assert_copies_consistent(edge_id);
        Ok(edge_id)
    }
}
