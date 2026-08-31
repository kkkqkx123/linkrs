use graphdb_core::{StorageError, StorageResult};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Status of a migration execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationStatus {
    Applied,
    RolledBack,
    Failed,
}

impl std::fmt::Display for MigrationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationStatus::Applied => write!(f, "applied"),
            MigrationStatus::RolledBack => write!(f, "rolled_back"),
            MigrationStatus::Failed => write!(f, "failed"),
        }
    }
}

/// One row of the `migration_history` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationHistoryRecord {
    pub id: u64,
    pub space: String,
    pub label: String,
    pub is_edge: bool,
    pub from_version: u64,
    pub to_version: u64,
    pub plan_hash: String,
    pub safety_level: String,
    pub steps_count: usize,
    pub rows_migrated: u64,
    pub status: MigrationStatus,
    pub applied_at: u64,
    pub completed_at: Option<u64>,
    pub error_message: Option<String>,
}

impl MigrationHistoryRecord {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        space: String,
        label: String,
        is_edge: bool,
        from_version: u64,
        to_version: u64,
        plan_hash: String,
        safety_level: String,
        steps_count: usize,
        rows_migrated: u64,
        status: MigrationStatus,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Self {
            id: 0,
            space,
            label,
            is_edge,
            from_version,
            to_version,
            plan_hash,
            safety_level,
            steps_count,
            rows_migrated,
            status,
            applied_at: now,
            completed_at: Some(now),
            error_message: None,
        }
    }
}

/// In-memory manager for migration history. Persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MigrationHistoryManager {
    records: Vec<MigrationHistoryRecord>,
    next_id: u64,
    #[serde(skip)]
    index: HashMap<(String, String, bool, u64), usize>,
}

impl MigrationHistoryManager {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            next_id: 1,
            index: HashMap::new(),
        }
    }

    fn rebuild_index(&mut self) {
        self.index.clear();
        for (idx, r) in self.records.iter().enumerate() {
            self.index
                .insert((r.space.clone(), r.label.clone(), r.is_edge, r.to_version), idx);
        }
        if let Some(max_id) = self.records.iter().map(|r| r.id).max() {
            self.next_id = max_id + 1;
        } else {
            self.next_id = 1;
        }
    }

    pub fn record(&mut self, mut rec: MigrationHistoryRecord) -> StorageResult<()> {
        let key = (rec.space.clone(), rec.label.clone(), rec.is_edge, rec.to_version);
        if self.index.contains_key(&key) {
            return Err(StorageError::already_exists(format!(
                "migration_history unique violation: space={} label={} is_edge={} to_version={}",
                rec.space, rec.label, rec.is_edge, rec.to_version
            )));
        }
        rec.id = self.next_id;
        self.next_id += 1;
        let idx = self.records.len();
        self.records.push(rec);
        self.index.insert(key, idx);
        Ok(())
    }

    pub fn list(
        &self,
        space: &str,
        label: &str,
        is_edge: bool,
    ) -> Vec<MigrationHistoryRecord> {
        self.records
            .iter()
            .filter(|r| r.space == space && r.label == label && r.is_edge == is_edge)
            .cloned()
            .collect()
    }

    pub fn list_all(&self) -> Vec<MigrationHistoryRecord> {
        self.records.clone()
    }

    pub fn get_applied_versions(
        &self,
        space: &str,
        label: &str,
        is_edge: bool,
    ) -> HashSet<u64> {
        self.records
            .iter()
            .filter(|r| {
                r.space == space
                    && r.label == label
                    && r.is_edge == is_edge
                    && r.status == MigrationStatus::Applied
            })
            .map(|r| r.to_version)
            .collect()
    }

    pub fn get_applied_versions_sorted(
        &self,
        space: &str,
        label: &str,
        is_edge: bool,
    ) -> Vec<u64> {
        let mut v: Vec<u64> = self.get_applied_versions(space, label, is_edge).into_iter().collect();
        v.sort_unstable();
        v
    }

    pub fn find(
        &self,
        space: &str,
        label: &str,
        is_edge: bool,
        to_version: u64,
    ) -> Option<MigrationHistoryRecord> {
        let key = (space.to_string(), label.to_string(), is_edge, to_version);
        self.index.get(&key).and_then(|&idx| self.records.get(idx).cloned())
    }

    pub fn save_to_file(&self, path: &Path) -> StorageResult<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StorageError::io_error(e.to_string()))?;
        }
        let json = serde_json::to_string_pretty(&self.records)
            .map_err(|e| StorageError::serialize_error(e.to_string()))?;
        std::fs::write(path, json).map_err(|e| StorageError::io_error(e.to_string()))?;
        Ok(())
    }

    pub fn load_from_file(path: &Path) -> StorageResult<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let data = std::fs::read_to_string(path).map_err(|e| StorageError::io_error(e.to_string()))?;
        if data.trim().is_empty() {
            return Ok(Self::new());
        }
        let records: Vec<MigrationHistoryRecord> =
            serde_json::from_str(&data).map_err(|e| StorageError::deserialize_error(e.to_string()))?;
        let mut mgr = Self {
            records,
            next_id: 1,
            index: HashMap::new(),
        };
        mgr.rebuild_index();
        Ok(mgr)
    }

    pub fn clear(&mut self) {
        self.records.clear();
        self.index.clear();
        self.next_id = 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_query() {
        let mut mgr = MigrationHistoryManager::new();
        let rec = MigrationHistoryRecord::new(
            "s".into(),
            "l".into(),
            false,
            1,
            2,
            "hash".into(),
            "SAFE".into(),
            1,
            10,
            MigrationStatus::Applied,
        );
        mgr.record(rec).unwrap();
        assert_eq!(mgr.get_applied_versions("s", "l", false).len(), 1);
        assert!(mgr.get_applied_versions("s", "l", true).is_empty());
    }

    #[test]
    fn test_unique_violation() {
        let mut mgr = MigrationHistoryManager::new();
        let rec = MigrationHistoryRecord::new(
            "s".into(),
            "l".into(),
            false,
            1,
            2,
            "hash".into(),
            "SAFE".into(),
            1,
            10,
            MigrationStatus::Applied,
        );
        mgr.record(rec.clone()).unwrap();
        let err = mgr.record(rec).unwrap_err();
        assert_eq!(err.kind(), graphdb_core::error::storage::StorageErrorKind::AlreadyExists);
    }

    #[test]
    fn test_save_load() {
        let mut mgr = MigrationHistoryManager::new();
        mgr.record(MigrationHistoryRecord::new(
            "s".into(),
            "l".into(),
            false,
            1,
            2,
            "h".into(),
            "SAFE".into(),
            1,
            5,
            MigrationStatus::Applied,
        ))
        .unwrap();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        mgr.save_to_file(tmp.path()).unwrap();
        let loaded = MigrationHistoryManager::load_from_file(tmp.path()).unwrap();
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.get_applied_versions("s", "l", false).len(), 1);
    }
}
