use super::{CsrWithProperties, ExportedRow};
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageResult, Value};

impl CsrWithProperties {
    /// Export one edge row for per-group property sharding.
    ///
    /// Returns the row visibility plus current values for every schema
    /// column (nulls as `None`). Unknown edges yield `None`. The export
    /// carries plain values only; version history stays memory-only and is
    /// collapsed on checkpoint, matching the whole-file dump contract.
    pub fn export_row(&self, edge_id: EdgeId) -> Option<ExportedRow> {
        let pos = self.mapped_row(edge_id)?;
        let vis = *self.visibility.get(pos)?;
        let values = self
            .property_schema
            .iter()
            .enumerate()
            .map(|(idx, schema)| {
                let current = self.property_columns.get(idx).and_then(|col| col.get(pos));
                (schema.name.clone(), current)
            })
            .collect();
        Some((vis.create_ts, vis.delete_ts, values))
    }

    /// Import one exported row into this store for shard merge.
    ///
    /// Unknown columns in `values` are skipped so shards written under an
    /// older published schema still merge; missing columns keep their
    /// defaults via the fresh insert path. Nulls are materialized
    /// explicitly so a shard null never becomes a default on merge.
    pub fn import_row(
        &mut self,
        edge_id: EdgeId,
        create_ts: Timestamp,
        delete_ts: Option<Timestamp>,
        values: &[(String, Option<Value>)],
    ) -> StorageResult<()> {
        if self.mapped_row(edge_id).is_some() {
            return Ok(());
        }
        let mut present: Vec<(usize, Value)> = Vec::new();
        let mut nulls: Vec<String> = Vec::new();
        for (name, opt) in values {
            let Some(&idx) = self.column_index.get(name.as_str()) else {
                continue;
            };
            match opt {
                Some(value) => present.push((idx, value.clone())),
                None => nulls.push(name.clone()),
            }
        }
        self.insert_for_edge_at(edge_id, &present, create_ts)?;
        for name in nulls {
            let _ = self.set_property_for_edge(edge_id, &name, None, create_ts);
        }
        if let Some(delete) = delete_ts {
            let _ = self.mark_deleted(edge_id, delete);
        }
        Ok(())
    }
}
