use crate::core::types::{LabelId, Timestamp};

use super::GraphStorageContext;

impl GraphStorageContext {
    pub fn scan_vertices(&self, label: LabelId, ts: Timestamp) -> Option<Vec<crate::storage::vertex::VertexRecord>> {
        if !self.persistent.is_open.load(std::sync::atomic::Ordering::Acquire) {
            return None;
        }
        let vertex_tables = self.persistent.data_store.vertex_tables().read();
        vertex_tables.get(&label).map(|t| t.scan(ts).collect())
    }

    pub fn scan_edges(
        &self,
        src_label: LabelId,
        dst_label: LabelId,
        edge_label: LabelId,
        ts: Timestamp,
    ) -> Vec<crate::storage::edge::EdgeRecord> {
        use crate::storage::engine::data_store::EdgeTableKey;
        self.persistent
            .data_store
            .edge_tables()
            .read()
            .get(&EdgeTableKey::new(src_label, dst_label, edge_label))
            .map(|t| t.scan(ts))
            .unwrap_or_default()
    }

    pub fn scan_edges_by_label(&self, edge_label: LabelId, ts: Timestamp) -> Vec<crate::storage::edge::EdgeRecord> {
        let edge_tables = self.persistent.data_store.edge_tables().read();
        let mut records = Vec::new();

        for table in edge_tables
            .values()
            .filter(|table| table.label() == edge_label)
        {
            records.extend(table.scan(ts));
        }

        records
    }

    pub fn total_vertex_count(&self) -> usize {
        self.persistent
            .data_store
            .vertex_tables()
            .read()
            .values()
            .map(|t| t.total_count())
            .sum()
    }

    pub fn total_edge_count(&self) -> usize {
        self.persistent
            .data_store
            .edge_tables()
            .read()
            .values()
            .map(|t| t.edge_count() as usize)
            .sum()
    }

    pub fn collect_all_edge_records(
        &self,
        ts: Timestamp,
    ) -> Vec<(LabelId, LabelId, LabelId, crate::storage::edge::EdgeRecord)> {
        use crate::storage::engine::data_store::EdgeTableKey;
        let edge_tables = self.persistent.data_store.edge_tables().read();
        let mut records = Vec::new();
        for (
            EdgeTableKey {
                src_label,
                dst_label,
                edge_label,
            },
            table,
        ) in &*edge_tables
        {
            for edge_record in table.scan(ts) {
                records.push((*src_label, *dst_label, *edge_label, edge_record));
            }
        }
        records
    }
}
