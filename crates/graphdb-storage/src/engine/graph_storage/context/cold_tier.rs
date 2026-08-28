use std::collections::HashSet;
use std::path::PathBuf;

use crate::engine::config::ColdTierConfig;
use graphdb_core::types::LabelId;
use graphdb_core::StorageResult;

use super::GraphStorageContext;

impl GraphStorageContext {
    /// Automatic cold-hot tiering: export idle, oversized edge labels to
    /// `.lkcs` files and evict the frozen rows from the hot store.
    ///
    /// A label is eligible when it has more than `trigger_row_count` live
    /// edges, has seen no writes for `trigger_idle_seconds`, and has fewer
    /// than `max_cold_snapshots_per_label` snapshots already. The snapshot
    /// captures the table state at the current read timestamp, excluding the
    /// `preserve_recent_edges` newest edges; the same set is then deleted
    /// from the hot store, so reads at or after the snapshot timestamp fall
    /// through to the cold tier while older reads keep the hot rows.
    ///
    /// Returns the total number of evicted edges.
    pub(crate) fn maybe_freeze_cold_tier(&self) -> StorageResult<usize> {
        let cfg = &self.persistent.config.cold_tier;
        if !cfg.enabled {
            return Ok(0);
        }

        let candidates = self.persistent.data_store.with_edge_tables(|tables| {
            tables
                .values()
                .map(|arc| {
                    let table = arc.read();
                    (table.label(), table.edge_count())
                })
                .filter(|(label, count)| {
                    *count > cfg.trigger_row_count
                        && self.edge_idle_seconds(*label) >= cfg.trigger_idle_seconds
                })
                .collect::<Vec<(LabelId, u64)>>()
        });

        let mut seen_labels = HashSet::new();
        let mut total_evicted = 0usize;
        for (label, edge_count) in candidates {
            if !seen_labels.insert(label) {
                continue;
            }
            let snapshot_count = self
                .cold_snapshots()
                .read()
                .get(&label)
                .map(|snapshots| snapshots.len())
                .unwrap_or(0);
            if snapshot_count >= cfg.max_cold_snapshots_per_label {
                continue;
            }
            match self.freeze_cold_tier_label(label, edge_count, cfg) {
                Ok(evicted) => total_evicted += evicted,
                Err(err) => {
                    log::warn!("Cold-tier freeze failed for label {}: {}", label, err);
                }
            }
        }
        if total_evicted > 0 {
            log::info!(
                "Cold-tier freeze: {} edges evicted across {} label(s)",
                total_evicted,
                seen_labels.len()
            );
        }
        Ok(total_evicted)
    }

    fn freeze_cold_tier_label(
        &self,
        label: LabelId,
        edge_count: u64,
        cfg: &ColdTierConfig,
    ) -> StorageResult<usize> {
        let ts = self.get_read_timestamp();
        let snapshot_dir = if cfg.snapshot_dir.as_os_str().is_empty() {
            self.persistent
                .layout
                .work_dir()
                .clone()
                .unwrap_or_else(|| PathBuf::from("/tmp/linkrs_cold"))
                .join("cold_snapshots")
        } else {
            cfg.snapshot_dir.clone()
        };
        std::fs::create_dir_all(&snapshot_dir)?;

        let path = self.persistent.data_store.with_edge_tables(
            |tables| -> StorageResult<(PathBuf, crate::cold::ColdSnapshot, u64)> {
                let arc = tables
                    .values()
                    .find(|arc| arc.read().label() == label)
                    .ok_or_else(|| {
                        graphdb_core::StorageError::label_not_found(format!(
                            "edge label {} disappeared before freeze",
                            label
                        ))
                    })?
                    .clone();
                let mut table = arc.write();
                let name = table.schema().label_name.clone();
                let path = snapshot_dir.join(format!("{}_{}.lkcs", name, ts));
                let snapshot = table.export_snapshot_file_with_retention(
                    ts,
                    cfg.preserve_recent_edges,
                    &path,
                )?;
                let evicted = table.freeze_edges_before(ts, cfg.preserve_recent_edges)?;
                Ok((path, snapshot, evicted))
            },
        )?;

        self.load_cold_snapshot(path.1);
        self.mark_edge_modified(label);
        log::info!(
            "Cold-tier freeze label {} ({} edges): snapshot {} written, {} edges evicted",
            label,
            edge_count,
            path.0.display(),
            path.2
        );
        Ok(path.2 as usize)
    }
}
