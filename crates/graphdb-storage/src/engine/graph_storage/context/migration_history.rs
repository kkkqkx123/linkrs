use std::sync::Arc;

use super::GraphStorageContext;

impl GraphStorageContext {
    pub(crate) fn migration_history(
        &self,
    ) -> &Arc<parking_lot::RwLock<crate::migration_history::MigrationHistoryManager>> {
        &self.persistent.migration_history
    }

    pub fn record_migration_history(
        &self,
        record: crate::migration_history::MigrationHistoryRecord,
    ) -> graphdb_core::StorageResult<()> {
        let mut mgr = self.persistent.migration_history.write();
        mgr.record(record.clone())?;
        if let Some(paths) = self.storage_paths() {
            let path = paths.migration_history_file();
            if let Err(e) = mgr.save_to_file(&path) {
                log::warn!("Failed to persist migration_history: {}", e);
            }
        }
        Ok(())
    }

    pub fn list_migration_history(
        &self,
        space: &str,
        label: &str,
        is_edge: bool,
    ) -> Vec<crate::migration_history::MigrationHistoryRecord> {
        self.persistent
            .migration_history
            .read()
            .list(space, label, is_edge)
    }

    pub fn list_all_migration_history(
        &self,
    ) -> Vec<crate::migration_history::MigrationHistoryRecord> {
        self.persistent.migration_history.read().list_all()
    }

    pub fn get_applied_versions(&self, space: &str, label: &str, is_edge: bool) -> Vec<u64> {
        self.persistent
            .migration_history
            .read()
            .get_applied_versions_sorted(space, label, is_edge)
    }

    pub(crate) fn load_migration_history(&self) -> graphdb_core::StorageResult<()> {
        if let Some(work_dir) = self.work_dir().as_ref().cloned() {
            use crate::engine::paths::StoragePaths;
            use graphdb_sync::checkpoint_manifest::CheckpointManifestManager;
            let checkpoint_root = work_dir.join("checkpoint");
            let mut candidate_paths = Vec::new();
            if checkpoint_root.exists() {
                if let Ok(Some(manifest)) =
                    CheckpointManifestManager::new(checkpoint_root.join("manifests")).load_latest()
                {
                    candidate_paths.push(
                        StoragePaths::new(manifest.storage_snapshot.path).migration_history_file(),
                    );
                }
            }
            if let Some(paths) = self.storage_paths() {
                candidate_paths.push(paths.migration_history_file());
            }
            for path in candidate_paths {
                if path.exists() {
                    let mgr =
                        crate::migration_history::MigrationHistoryManager::load_from_file(&path)?;
                    *self.persistent.migration_history.write() = mgr;
                    break;
                }
            }
        } else if let Some(paths) = self.storage_paths() {
            let path = paths.migration_history_file();
            if path.exists() {
                let mgr = crate::migration_history::MigrationHistoryManager::load_from_file(&path)?;
                *self.persistent.migration_history.write() = mgr;
            }
        }
        Ok(())
    }

    pub(crate) fn save_migration_history(&self) -> graphdb_core::StorageResult<()> {
        if let Some(paths) = self.storage_paths() {
            let path = paths.migration_history_file();
            self.persistent
                .migration_history
                .read()
                .save_to_file(&path)?;
        }
        Ok(())
    }
}
