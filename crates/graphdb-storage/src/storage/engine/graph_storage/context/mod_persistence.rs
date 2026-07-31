use crate::core::metadata::IndexMetadataManager;
use crate::core::types::LabelId;
use crate::core::StorageResult;
use crate::storage::engine::data_store::EdgeTableKey;
use std::path::Path;

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

        match self.trigger_background_freeze() {
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

        {
            self.persistent
                .data_store
                .with_vertex_tables_mut(|vertex_tables| {
                    for (label_id, table) in vertex_tables.iter() {
                        let table_dir = vertex_dir.join(format!("label_{}", label_id));
                        table.flush(&table_dir, compression)?;
                    }
                    Ok(())
                })?;
        }

        let edge_dir = data_dir.join("edges");
        fs::create_dir_all(&edge_dir)?;

        {
            let ts = self.get_read_timestamp();
            self.persistent
                .data_store
                .for_all_edge_partitions_mut(|key, table| {
                    let table_dir = edge_dir.join(format!(
                        "{}_{}_{}",
                        key.src_label, key.dst_label, key.edge_label
                    ));
                    table.maybe_compact_for_flush(ts, 2.0);
                    table.flush(&table_dir, compression)?;
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

    pub(crate) fn restore_from_checkpoint(&self, checkpoint_dir: &Path) -> StorageResult<()> {
        use std::fs;

        let checkpoint_paths = crate::storage::engine::paths::StoragePaths::new(checkpoint_dir);

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
                                                table.load(&path)?;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Ok::<(), crate::core::StorageError>(())
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

        Ok(())
    }
}
