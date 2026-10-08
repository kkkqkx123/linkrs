use parking_lot::RwLock;

use super::core::{VertexTable, VertexTableConfig};
use super::sharded::routing::ShardLayout;

pub(crate) mod maintenance;
pub(crate) mod persistence;
mod read;
mod reshard;
pub(crate) mod routing;
mod schema;
pub(crate) mod write;

pub(crate) use write::CommitApplied;

pub struct ShardedVertexTable {
    shards: Vec<RwLock<VertexTable>>,
    layout: ShardLayout,
    label: linkrs_core::types::LabelId,
    label_name: String,
    /// Redistribution generation of this table lineage. Fresh tables are
    /// generation zero; each offline redistribution bumps it. Persisted in
    /// the table manifest and pinned in the commit manifest so the open
    /// path refuses checkpoints mixed in from another generation.
    generation: u64,
    /// Wall-clock milliseconds of the last full baseline flush. Feeds the
    /// flush trigger's baseline-age branch; zero means never flushed.
    last_full_flush_ms: std::sync::atomic::AtomicU64,
}

impl ShardedVertexTable {
    pub fn new(
        label: linkrs_core::types::LabelId,
        label_name: String,
        schema: crate::vertex::VertexSchema,
    ) -> Self {
        Self::with_estimate(label, label_name, schema, 1, None)
    }

    /// Build a new table from an estimated row count plus a parallelism cap.
    /// Small estimates stay on one shard; large estimates keep the
    /// parallelism-shaped layout. Opened tables ignore this and adopt the
    /// layout pinned in their manifest.
    pub fn with_estimate(
        label: linkrs_core::types::LabelId,
        label_name: String,
        schema: crate::vertex::VertexSchema,
        parallelism_shards: usize,
        estimated_rows: Option<u64>,
    ) -> Self {
        Self::with_layout(
            label,
            label_name,
            schema,
            ShardLayout::for_new_table_with_estimate(parallelism_shards, estimated_rows),
            0,
        )
    }

    /// Build a table with an explicit shard count. Test and offline
    /// redistribution only; production creation goes through `with_estimate`.
    #[allow(dead_code)]
    pub fn with_config(
        label: linkrs_core::types::LabelId,
        label_name: String,
        schema: crate::vertex::VertexSchema,
        num_shards: usize,
    ) -> Self {
        Self::with_layout(
            label,
            label_name,
            schema,
            ShardLayout::for_new_table(num_shards),
            0,
        )
    }

    /// Build a table under an explicit versioned layout. New tables use
    /// [`ShardLayout::for_new_table_with_estimate`] with generation zero;
    /// opened tables use the layout and generation pinned in their manifest.
    pub(crate) fn with_layout(
        label: linkrs_core::types::LabelId,
        label_name: String,
        schema: crate::vertex::VertexSchema,
        layout: ShardLayout,
        generation: u64,
    ) -> Self {
        let mut shards = Vec::with_capacity(layout.num_shards);
        for _ in 0..layout.num_shards {
            shards.push(RwLock::new(VertexTable::with_config(
                label,
                label_name.clone(),
                schema.clone(),
                VertexTableConfig::default(),
            )));
        }
        Self {
            shards,
            layout,
            label,
            label_name,
            generation,
            last_full_flush_ms: std::sync::atomic::AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    pub(crate) fn verify_invariants(&self) -> linkrs_core::StorageResult<()> {
        use linkrs_core::error::storage::StorageErrorKind;

        for shard in &self.shards {
            let table = shard.read();
            let id_count = table.id_indexer.len();

            for (key, idx) in table.id_indexer.iter() {
                let start_ts = table.timestamps.read().get_start_ts(idx);
                if start_ts.is_none() {
                    return Err(linkrs_core::StorageError::new(
                        StorageErrorKind::StorageError,
                        format!("ID {} for key {:?} missing in timestamps", idx, key),
                    ));
                }
            }

            for idx in 0..table.timestamps.read().size() {
                if let Some(_start_ts) = table.timestamps.read().get_start_ts(idx as u32) {
                    let key = table.id_indexer.get_key(idx as u32);
                    if key.is_none() {
                        return Err(linkrs_core::StorageError::new(
                            StorageErrorKind::StorageError,
                            format!("Timestamp entry {} missing in id_indexer", idx),
                        ));
                    }
                }
            }

            // Stable row ids leave holes: absorbed deletes keep their
            // column rows and timestamp capacity until free-stack reuse,
            // so the column store may be wider than the live key count.
            if table.columns.row_count() < id_count {
                return Err(linkrs_core::StorageError::new(
                    StorageErrorKind::StorageError,
                    format!(
                        "Column count ({}) below id_indexer.len() ({})",
                        table.columns.row_count(),
                        id_count
                    ),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
