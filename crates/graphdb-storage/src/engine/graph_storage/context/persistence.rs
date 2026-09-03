use crate::engine::data_store::EdgeTableKey;
use graphdb_core::metadata::IndexMetadataManager;
use graphdb_core::types::LabelId;
use graphdb_core::StorageResult;
use rayon::prelude::*;
use std::path::Path;
use std::sync::Arc;

use super::GraphStorageContext;

impl GraphStorageContext {
    pub(crate) fn register_loaded_native_indexes(&self) -> StorageResult<()> {
        let spaces = self.persistent.schema_manager.list_spaces()?;
        let index_manager = self.persistent.index_data_manager.write();
        for space in spaces {
            for index in self
                .persistent
                .index_metadata_manager
                .list_tag_indexes(space.space_id)?
            {
                index_manager.register_native_index(space.space_id, &index)?;
            }
            for index in self
                .persistent
                .index_metadata_manager
                .list_edge_indexes(space.space_id)?
            {
                index_manager.register_native_index(space.space_id, &index)?;
            }
        }
        Ok(())
    }

    pub(crate) fn flush_tables_to_dir(&self, data_dir: &Path) -> StorageResult<()> {
        use std::fs;

        match self.trigger_background_maintenance() {
            Ok(()) => {
                if let Some(stats) = self.get_freeze_stats() {
                    if stats.freeze_count > 0 {
                        log::info!(
                            "Pre-flush freeze: {} edges frozen in {} operations",
                            stats.total_frozen_edges,
                            stats.freeze_count
                        );
                    }
                }
            }
            Err(err) => {
                log::warn!("Pre-flush freeze failed: {}", err);
            }
        }

        let compression = self.persistent.config.flush_config.compression;
        let vertex_dir = data_dir.join("vertices");
        fs::create_dir_all(&vertex_dir)?;

        // Vertex table flush. Scatter-gather: collect table references under a
        // brief catalog READ lock, then flush each table under its own shard
        // locks outside the catalog lock. Previously the whole flush (disk IO,
        // compression, serialization) ran inside the catalog WRITE lock,
        // freezing every transaction begin and DDL for the entire database.
        let vertex_tables: Vec<(
            LabelId,
            Arc<crate::vertex::vertex_table::ShardedVertexTable>,
        )> = self.persistent.data_store.with_vertex_tables(|tables| {
            tables
                .iter()
                .map(|(label_id, table)| (*label_id, table.clone()))
                .collect()
        });
        self.runtime.thread_pool.install(|| -> StorageResult<()> {
            vertex_tables.par_iter().try_for_each(|(label_id, table)| {
                let table_dir = vertex_dir.join(format!("label_{}", label_id));
                table.flush(&table_dir, compression)
            })?;
            Ok(())
        })?;

        let edge_dir = data_dir.join("edges");
        fs::create_dir_all(&edge_dir)?;

        {
            let ts = self.get_read_timestamp();
            let edge_tables: Vec<(
                EdgeTableKey,
                Arc<parking_lot::RwLock<crate::edge::EdgeStore>>,
            )> = self.persistent.data_store.with_edge_tables(|tables| {
                tables
                    .iter()
                    .map(|(key, arc)| (*key, arc.clone()))
                    .collect()
            });
            self.runtime.thread_pool.install(|| -> StorageResult<()> {
                edge_tables
                    .par_iter()
                    .try_for_each(|(key, edge_table)| -> StorageResult<()> {
                        let table_dir = edge_dir.join(format!(
                            "{}_{}_{}",
                            key.src_label, key.dst_label, key.edge_label
                        ));
                        let mut table = edge_table.write();
                        table.maybe_compact_for_flush(ts, 2.0);
                        table.flush(&table_dir, compression)?;
                        Ok(())
                    })?;
                Ok(())
            })?;
        }

        let index_dir = data_dir.join("indexes");
        fs::create_dir_all(&index_dir)?;
        self.persistent
            .index_data_manager
            .read()
            .flush(&index_dir)?;

        if let Some(persistence) = self.persistent.persistence.as_ref() {
            persistence
                .read()
                .wal_manager()
                .and_then(|w| w.read().sync().ok());
        }

        Ok(())
    }

    pub(crate) fn flush_tables_to_checkpoint(&self, data_dir: &Path) -> StorageResult<()> {
        use std::fs;

        match self.trigger_background_maintenance() {
            Ok(()) => {
                if let Some(stats) = self.get_freeze_stats() {
                    if stats.freeze_count > 0 {
                        log::info!(
                            "Pre-flush freeze: {} edges frozen in {} operations",
                            stats.total_frozen_edges,
                            stats.freeze_count
                        );
                    }
                }
            }
            Err(err) => {
                log::warn!("Pre-flush freeze failed: {}", err);
            }
        }

        let compression = self.persistent.config.flush_config.compression;
        let vertex_dir = data_dir.join("vertices");
        fs::create_dir_all(&vertex_dir)?;

        // Compute global dirty ratio to decide incremental vs full flush.
        let (global_dirty_ratio, global_total_dirty, global_total_pages) = {
            let vertex_tables = self.persistent.data_store.with_vertex_tables(|tables| {
                tables
                    .iter()
                    .map(|(label_id, table)| (*label_id, table.clone()))
                    .collect::<Vec<_>>()
            });
            if vertex_tables.is_empty() {
                (0.0, 0usize, 0usize)
            } else {
                let mut total_dirty = 0usize;
                let mut total_pages = 0usize;
                for (_, table) in &vertex_tables {
                    total_dirty += table.total_dirty_pages();
                    total_pages += table.total_pages();
                }
                let ratio = if total_pages == 0 {
                    0.0
                } else {
                    total_dirty as f64 / total_pages as f64
                };
                (ratio, total_dirty, total_pages)
            }
        };
        let strategy =
            crate::persistence::dirty_page::select_checkpoint_strategy(global_dirty_ratio);
        if let Some(stats) = self.persistent.stats_manager.as_ref() {
            stats.record_dirty_pages(global_total_dirty as u64, global_total_pages as u64);
            stats.record_checkpoint_strategy_by_name(strategy.as_str());
        }
        log::info!(
            "Flush strategy selected: {:?} (dirty_ratio={:.3})",
            strategy,
            global_dirty_ratio
        );

        let vertex_tables: Vec<(
            LabelId,
            Arc<crate::vertex::vertex_table::ShardedVertexTable>,
        )> = self.persistent.data_store.with_vertex_tables(|tables| {
            tables
                .iter()
                .map(|(label_id, table)| (*label_id, table.clone()))
                .collect()
        });
        let use_incremental = matches!(
            strategy,
            crate::persistence::dirty_page::CheckpointStrategy::Incremental
        ) && global_dirty_ratio < 0.1
            && global_dirty_ratio > 0.0;

        // Collect dirty pages for incremental meta
        let (all_dirty_pages, total_pages) = {
            let mut pages = Vec::new();
            let mut total = 0usize;
            for (_, table) in &vertex_tables {
                total += table.total_pages();
                pages.extend(table.collect_dirty_pages());
            }
            (pages, total)
        };

        self.runtime.thread_pool.install(|| -> StorageResult<()> {
            vertex_tables.par_iter().try_for_each(|(label_id, table)| {
                let table_dir = vertex_dir.join(format!("label_{}", label_id));
                if use_incremental {
                    table.flush_incremental(&table_dir, compression)
                } else {
                    table.flush(&table_dir, compression)
                }
            })?;
            Ok(())
        })?;

        // Persist incremental checkpoint meta if incremental selected
        if use_incremental {
            // base checkpoint is latest published sequence
            let base_checkpoint_id = self.persistent.persistence.as_ref().and_then(|p| {
                p.read()
                    .manifest_manager
                    .load_latest()
                    .ok()
                    .flatten()
                    .map(|m| m.checkpoint_id)
            });
            let meta = crate::persistence::dirty_page::IncrementalCheckpointMeta {
                base_checkpoint_id,
                dirty_pages: all_dirty_pages.clone(),
                page_checksums: std::collections::HashMap::new(),
                total_pages,
                dirty_ratio: global_dirty_ratio,
                strategy,
            };
            // Write incremental.meta alongside data_dir (checkpoint root = data_dir parent)
            if let Some(parent) = data_dir.parent() {
                let meta_path = parent.join("incremental.meta");
                if let Ok(json) = serde_json::to_string_pretty(&meta) {
                    let _ = std::fs::write(&meta_path, json.as_bytes());
                }
                // Also clear global dirty after successful incremental persist
                // (per-table clear already done in flush_incremental, but ensure)
                for (_, table) in &vertex_tables {
                    table.clear_dirty();
                }
            }
            if let Some(stats) = self.persistent.stats_manager.as_ref() {
                stats.record_incremental_checkpoint(
                    std::time::Duration::from_micros(all_dirty_pages.len() as u64 * 100),
                    total_pages as u64,
                );
            }
        } else if let Some(stats) = self.persistent.stats_manager.as_ref() {
            // Full/Hybrid still records ratio
            stats.record_incremental_checkpoint(std::time::Duration::ZERO, total_pages as u64);
        }

        let edge_dir = data_dir.join("edges");
        fs::create_dir_all(&edge_dir)?;

        {
            let ts = self.get_read_timestamp();
            let edge_tables: Vec<(
                EdgeTableKey,
                Arc<parking_lot::RwLock<crate::edge::EdgeStore>>,
            )> = self.persistent.data_store.with_edge_tables(|tables| {
                tables
                    .iter()
                    .map(|(key, arc)| (*key, arc.clone()))
                    .collect()
            });
            self.runtime.thread_pool.install(|| -> StorageResult<()> {
                edge_tables
                    .par_iter()
                    .try_for_each(|(key, edge_table)| -> StorageResult<()> {
                        let table_dir = edge_dir.join(format!(
                            "{}_{}_{}",
                            key.src_label, key.dst_label, key.edge_label
                        ));
                        let mut table = edge_table.write();
                        table.maybe_compact_for_flush(ts, 2.0);
                        table.flush(&table_dir, compression)?;
                        Ok(())
                    })?;
                Ok(())
            })?;
        }

        let index_dir = data_dir.join("indexes");
        fs::create_dir_all(&index_dir)?;
        self.persistent
            .index_data_manager
            .read()
            .flush(&index_dir)?;

        if let Some(persistence) = self.persistent.persistence.as_ref() {
            persistence
                .read()
                .wal_manager()
                .and_then(|w| w.read().sync().ok());
        }

        Ok(())
    }

    fn parse_base_checkpoint_id(dir: &Path) -> Option<u64> {
        use std::fs::File;
        use std::io::{BufRead, BufReader};
        let meta = dir.join("checkpoint.meta");
        let file = File::open(meta).ok()?;
        let reader = BufReader::new(file);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(val) = line.strip_prefix("base_checkpoint_id=") {
                if let Ok(id) = val.parse::<u64>() {
                    return Some(id);
                }
            }
        }
        None
    }

    pub(crate) fn restore_from_checkpoint(&self, checkpoint_dir: &Path) -> StorageResult<()> {
        use std::fs;

        // Handle incremental checkpoint chain: if this checkpoint is incremental,
        // first restore its base, then overlay delta pages.
        // If base restore or delta overlay fails, fall back to loading the
        // current checkpoint as a best-effort full snapshot.
        if let Some(base_id) = Self::parse_base_checkpoint_id(checkpoint_dir) {
            if let Some(parent) = checkpoint_dir.parent() {
                let base_path = parent.join(format!("checkpoint_{}", base_id));
                if base_path.exists() && base_path != checkpoint_dir {
                    match self.restore_from_checkpoint(&base_path) {
                        Ok(()) => {
                            // Base restored successfully; overlay incremental delta
                            let checkpoint_paths =
                                crate::engine::paths::StoragePaths::new(checkpoint_dir);
                            let vertex_dir = checkpoint_paths.vertices_dir();
                            if vertex_dir.exists() {
                                if let Err(e) = self.persistent.data_store.with_vertex_tables_mut(
                                    |vertex_tables| {
                                        for entry in fs::read_dir(&vertex_dir)? {
                                            let entry = entry?;
                                            let path = entry.path();
                                            if path.is_dir() {
                                                if let Some(dir_name) = path.file_name() {
                                                    if let Some(name_str) = dir_name.to_str() {
                                                        if let Some(label_str) =
                                                            name_str.strip_prefix("label_")
                                                        {
                                                            if let Ok(label_id) =
                                                                label_str.parse::<LabelId>()
                                                            {
                                                                if let Some(table) =
                                                                    vertex_tables.get(&label_id)
                                                                {
                                                                    // Corrupted delta pages are skipped internally with warn
                                                                    if let Err(err) =
                                                                        table.apply_delta_pages(&path)
                                                                    {
                                                                        log::warn!(
                                                                            "Failed to apply delta pages for label {} from {}: {}, continuing with base data",
                                                                            label_id,
                                                                            path.display(),
                                                                            err
                                                                        );
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Ok::<(), graphdb_core::StorageError>(())
                                    },
                                ) {
                                    log::warn!(
                                        "Failed to overlay vertex delta pages for incremental checkpoint {}: {}",
                                        checkpoint_dir.display(),
                                        e
                                    );
                                }
                            }
                            let edge_dir = checkpoint_paths.edges_dir();
                            if edge_dir.exists() {
                                for entry in fs::read_dir(&edge_dir)? {
                                    let entry = entry?;
                                    let path = entry.path();
                                    if path.is_dir() {
                                        if let Some(dir_name) = path.file_name() {
                                            if let Some(name_str) = dir_name.to_str() {
                                                let parts: Vec<&str> =
                                                    name_str.splitn(3, '_').collect();
                                                if parts.len() == 3 {
                                                    if let (
                                                        Ok(src_label),
                                                        Ok(dst_label),
                                                        Ok(edge_label),
                                                    ) = (
                                                        parts[0].parse::<LabelId>(),
                                                        parts[1].parse::<LabelId>(),
                                                        parts[2].parse::<LabelId>(),
                                                    ) {
                                                        let key = EdgeTableKey::new(
                                                            src_label, dst_label, edge_label,
                                                        );
                                                        let data_store =
                                                            &self.persistent.data_store;
                                                        if let Some(arc) =
                                                            data_store.try_get_edge_table_mut(&key)
                                                        {
                                                            let mut table = arc.write();
                                                            if let Err(err) = table.load(&path) {
                                                                log::warn!(
                                                                    "Failed to load edge table for incremental checkpoint {}: {}",
                                                                    path.display(),
                                                                    err
                                                                );
                                                            } else if let Some(stats) =
                                                                &self.persistent.stats_manager
                                                            {
                                                                table.set_stats_manager(
                                                                    stats.clone(),
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            let index_dir = checkpoint_paths.data_dir().join("indexes");
                            if index_dir.exists() {
                                if let Err(e) =
                                    self.persistent.index_data_manager.write().load(&index_dir)
                                {
                                    log::warn!(
                                        "Failed to load indexes for incremental checkpoint {}: {}",
                                        checkpoint_dir.display(),
                                        e
                                    );
                                }
                            }
                            if let Err(e) = self.register_loaded_native_indexes() {
                                log::warn!(
                                    "Failed to register indexes after incremental restore {}: {}",
                                    checkpoint_dir.display(),
                                    e
                                );
                            }
                            self.rebuild_vertex_id_domains();
                            return Ok(());
                        }
                        Err(e) => {
                            log::warn!(
                                "Failed to restore incremental base checkpoint {}: {}, falling back to current checkpoint {}",
                                base_path.display(),
                                e,
                                checkpoint_dir.display()
                            );
                        }
                    }
                }
            }
        }

        let checkpoint_paths = crate::engine::paths::StoragePaths::new(checkpoint_dir);

        let vertex_dir = checkpoint_paths.vertices_dir();
        if vertex_dir.exists() {
            self.persistent
                .data_store
                .with_vertex_tables_mut(|vertex_tables| {
                    for entry in fs::read_dir(&vertex_dir)? {
                        let entry = entry?;
                        let path = entry.path();
                        if path.is_dir() {
                            if let Some(dir_name) = path.file_name() {
                                if let Some(name_str) = dir_name.to_str() {
                                    if let Some(label_str) = name_str.strip_prefix("label_") {
                                        if let Ok(label_id) = label_str.parse::<LabelId>() {
                                            if let Some(table) = vertex_tables.get(&label_id) {
                                                table.as_ref().load(&path)?;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok::<(), graphdb_core::StorageError>(())
                })?;
        }

        let edge_dir = checkpoint_paths.edges_dir();
        if edge_dir.exists() {
            for entry in fs::read_dir(&edge_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() {
                    if let Some(dir_name) = path.file_name() {
                        if let Some(name_str) = dir_name.to_str() {
                            let parts: Vec<&str> = name_str.splitn(3, '_').collect();
                            if parts.len() == 3 {
                                if let (Ok(src_label), Ok(dst_label), Ok(edge_label)) = (
                                    parts[0].parse::<LabelId>(),
                                    parts[1].parse::<LabelId>(),
                                    parts[2].parse::<LabelId>(),
                                ) {
                                    let key = EdgeTableKey::new(src_label, dst_label, edge_label);
                                    let data_store = &self.persistent.data_store;
                                    if let Some(arc) = data_store.try_get_edge_table_mut(&key) {
                                        let mut table = arc.write();
                                        table.load(&path)?;
                                        if let Some(stats) = &self.persistent.stats_manager {
                                            table.set_stats_manager(stats.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Checkpoints place native index files below data/ because they are
        // flushed together with the table snapshot.
        let index_dir = checkpoint_paths.data_dir().join("indexes");
        if index_dir.exists() {
            self.persistent
                .index_data_manager
                .write()
                .load(&index_dir)?;
        }

        self.register_loaded_native_indexes()?;

        // Restored tables bypass the write-path domain accumulator; rebuild
        // the self-proven vertex-id evidence and bump the layout version so
        // cached plans that assumed an older layout are invalidated.
        self.rebuild_vertex_id_domains();

        Ok(())
    }
}
