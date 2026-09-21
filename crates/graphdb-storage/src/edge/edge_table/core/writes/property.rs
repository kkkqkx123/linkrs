use super::super::EdgeStore;
use crate::edge::edge_table::config::UpdateEdgePropertyByKeyParams;
use crate::edge::edge_table::wal;
use crate::types::PropertyId;
use graphdb_core::types::Timestamp;
use graphdb_core::{StorageError, StorageResult, Value};

impl EdgeStore {
    /// Single property point write with its own log entry.
    ///
    /// Own atomic unit outside topology batches: one WAL append plus one
    /// column write. Topology visibility still comes from the version
    /// authority; the column version chain only carries values.
    pub fn update_edge_property(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        prop_name: &str,
        value: &Value,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        if self.migration_pending_checkpoint {
            return Err(StorageError::invalid_operation(
                "table requires a checkpoint after record-form switch before further writes"
                    .to_string(),
            ));
        }

        // Validate property exists via cache
        let _ = self
            .property_index_cache
            .get(prop_name)
            .ok_or_else(|| StorageError::column_not_found(prop_name.to_string()))?;

        let dst_key = Self::edge_endpoint_key(dst, rank);
        if let Some(nbr) = self.merged_get_edge(&self.out_csr, src, dst_key, ts) {
            if let Some(dir) = self.wal_dir.clone() {
                wal::append_ops(
                    &dir,
                    &[wal::EdgeWalOp::PropertyUpdate {
                        src,
                        dst,
                        rank,
                        prop_name: prop_name.to_string(),
                        value: value.clone(),
                        ts,
                    }],
                )?;
            }
            if self.is_bundled() {
                self.write_bundled_property(src, dst, prop_name, value)?;
            } else {
                self.properties
                    .set_property_for_edge(nbr.edge_id, prop_name, Some(value.clone()), ts)
                    .map_err(|_| StorageError::column_not_found(prop_name.to_string()))?;
            }
            self.mark_properties_dirty_for_edge(src, dst);
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }

        Ok(false)
    }

    pub fn update_edge_property_by_key(
        &mut self,
        params: UpdateEdgePropertyByKeyParams,
    ) -> StorageResult<bool> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        if self.migration_pending_checkpoint {
            return Err(StorageError::invalid_operation(
                "table requires a checkpoint after record-form switch before further writes"
                    .to_string(),
            ));
        }

        let dst_key = Self::edge_endpoint_key(params.dst, params.rank);
        if let Some(nbr) = self.merged_get_edge(&self.out_csr, params.src, dst_key, params.ts) {
            if let Some(dir) = self.wal_dir.clone() {
                let prop_name = self
                    .properties
                    .column_name_by_prop_id(params.prop_id as i32)
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| format!("prop_id={}", params.prop_id));
                wal::append_ops(
                    &dir,
                    &[wal::EdgeWalOp::PropertyUpdate {
                        src: params.src,
                        dst: params.dst,
                        rank: params.rank,
                        prop_name,
                        value: params.value.clone(),
                        ts: params.ts,
                    }],
                )?;
            }
            if self.is_bundled() {
                let prop_name = self
                    .properties
                    .column_name_by_prop_id(params.prop_id as i32)
                    .map(|name| name.to_string())
                    .ok_or_else(|| {
                        StorageError::column_not_found(format!("prop_id={}", params.prop_id))
                    })?;
                self.write_bundled_property(params.src, params.dst, &prop_name, &params.value)?;
            } else {
                self.properties
                    .set_property_by_id_for_edge(
                        nbr.edge_id,
                        PropertyId(params.prop_id),
                        Some(params.value.clone()),
                        params.ts,
                    )
                    .map_err(|_| {
                        StorageError::column_not_found(format!("prop_id={}", params.prop_id))
                    })?;
            }
            self.mark_properties_dirty_for_edge(params.src, params.dst);

            let src_key = Self::edge_endpoint_key(params.src, params.rank);
            if let Some(ie_nbr) = self.merged_get_edge(&self.in_csr, params.dst, src_key, params.ts)
            {
                if nbr.edge_id != ie_nbr.edge_id {
                    return Err(StorageError::data_corruption(format!(
                        "edge_id mismatch: out_csr={}, in_csr={} at edge ({}, {})",
                        nbr.edge_id.0, ie_nbr.edge_id.0, params.src, params.dst
                    )));
                }
            }
            self.maybe_run_auto_maintenance();
            return Ok(true);
        }

        Ok(false)
    }
}
