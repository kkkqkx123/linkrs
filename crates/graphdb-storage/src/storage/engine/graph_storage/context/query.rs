use crate::core::types::{LabelId, Timestamp};
use crate::core::StorageResult;

use super::GraphStorageContext;

impl GraphStorageContext {
    pub fn scan_vertices(
        &self,
        label: LabelId,
        ts: Timestamp,
    ) -> Option<Vec<crate::storage::vertex::VertexRecord>> {
        if !self
            .persistent
            .is_open
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return None;
        }
        // Lazily register the statement snapshot for this label.
        self.ensure_vertex_snapshot_registered(label);
        self.persistent
            .data_store
            .catalog_read_snapshot()
            .with_vertex_tables(|tables| tables.get(&label).map(|table| table.scan(ts)))
    }

    pub fn total_vertex_count(&self) -> usize {
        self.persistent
            .data_store
            .catalog_read_snapshot()
            .with_vertex_tables(|tables| tables.values().map(|table| table.total_count()).sum())
    }

    pub fn total_edge_count(&self) -> usize {
        self.persistent
            .data_store
            .catalog_read_snapshot()
            .with_edge_tables(|tables| {
                tables
                    .values()
                    .map(|arc| arc.read().edge_count() as usize)
                    .sum()
            })
    }

    pub fn collect_all_edge_records(
        &self,
        ts: Timestamp,
    ) -> Vec<(LabelId, LabelId, LabelId, crate::storage::edge::EdgeRecord)> {
        use crate::storage::engine::data_store::EdgeTableKey;
        self.persistent
            .data_store
            .catalog_read_snapshot()
            .with_edge_tables(|tables| {
                let mut records = Vec::new();
                for (
                    EdgeTableKey {
                        src_label,
                        dst_label,
                        edge_label,
                    },
                    arc,
                ) in tables
                {
                    let table = arc.read();
                    for edge_record in table.scan(ts) {
                        records.push((*src_label, *dst_label, *edge_label, edge_record));
                    }
                }
                records
            })
    }

    // ── Edge Property Index ──

    pub fn enable_edge_property_index(
        &self,
        src_label: LabelId,
        dst_label: LabelId,
        edge_label: LabelId,
        pool_capacity: u64,
    ) -> StorageResult<()> {
        use crate::storage::engine::data_store::EdgeTableKey;
        self.persistent.data_store.with_single_edge_table_mut(
            &EdgeTableKey::new(src_label, dst_label, edge_label),
            |table| table.enable_property_index(pool_capacity),
        )
    }

    pub fn has_edge_property_index(
        &self,
        src_label: LabelId,
        dst_label: LabelId,
        edge_label: LabelId,
    ) -> bool {
        use crate::storage::engine::data_store::EdgeTableKey;
        self.persistent
            .data_store
            .catalog_read_snapshot()
            .with_edge_tables(|tables| {
                tables
                    .get(&EdgeTableKey::new(src_label, dst_label, edge_label))
                    .map(|arc| arc.read().has_property_index())
                    .unwrap_or(false)
            })
    }

    pub fn disable_edge_property_index(
        &self,
        src_label: LabelId,
        dst_label: LabelId,
        edge_label: LabelId,
    ) -> StorageResult<()> {
        use crate::storage::engine::data_store::EdgeTableKey;
        self.persistent.data_store.with_single_edge_table_mut(
            &EdgeTableKey::new(src_label, dst_label, edge_label),
            |table| {
                table.disable_property_index();
                Ok(())
            },
        )
    }

    /// Look up edges whose `prop_name` value falls in `[value_lower, value_upper)`.
    #[allow(clippy::too_many_arguments)]
    pub fn lookup_edges_by_property_range(
        &self,
        src_label: LabelId,
        dst_label: LabelId,
        edge_label: LabelId,
        prop_name: &str,
        value_lower: &[u8],
        value_upper: &[u8],
        ts: Timestamp,
    ) -> Vec<crate::storage::edge::EdgeRecord> {
        use crate::storage::engine::data_store::EdgeTableKey;
        self.persistent
            .data_store
            .catalog_read_snapshot()
            .with_edge_tables(|tables| {
                tables
                    .get(&EdgeTableKey::new(src_label, dst_label, edge_label))
                    .map(|arc| {
                        let table = arc.read();
                        table
                            .lookup_edges_by_property_range(prop_name, value_lower, value_upper)
                            .into_iter()
                            .filter_map(|(src, dst, rank)| table.get_edge(src, dst, rank, ts))
                            .collect()
                    })
                    .unwrap_or_default()
            })
    }
}
