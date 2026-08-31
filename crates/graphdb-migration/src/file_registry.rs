use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::MigrationError;
use crate::plan::MigrationPlan;

/// A single migration file entry tracked by the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationFileEntry {
    pub id: String,
    pub space: String,
    pub label: String,
    pub is_edge: bool,
    pub from_version: u64,
    pub to_version: u64,
    pub plan_hash: String,
    pub file_path: PathBuf,
    pub applied_at: Option<u64>,
    pub checksum: String,
}

/// Registry that manages migration files on disk.
pub struct MigrationFileRegistry {
    base_dir: PathBuf,
}

impl MigrationFileRegistry {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Create a new migration file from a plan.
    pub fn create_migration_file(
        &self,
        plan: &MigrationPlan,
        sequence: u32,
    ) -> Result<PathBuf, MigrationError> {
        let dir = self
            .base_dir
            .join(&plan.target.space)
            .join(&plan.target.label);
        std::fs::create_dir_all(&dir)
            .map_err(|e| MigrationError::Plan(format!("failed to create migration dir: {e}")))?;

        let step_name = plan
            .steps
            .first()
            .map(|s| sanitize_name(&s.description()))
            .unwrap_or_else(|| "empty".to_string());

        let filename = format!("V{sequence:03}__{step_name}.json");
        let path = dir.join(&filename);

        let content = serde_json::to_string_pretty(plan)
            .map_err(|e| MigrationError::Plan(format!("failed to serialize plan: {e}")))?;

        std::fs::write(&path, content)
            .map_err(|e| MigrationError::Plan(format!("failed to write migration file: {e}")))?;

        Ok(path)
    }

    /// List all migration files for a given label, sorted by version.
    pub fn list_files(
        &self,
        space: &str,
        label: &str,
        _is_edge: bool,
    ) -> Result<Vec<MigrationFileEntry>, MigrationError> {
        let dir = self.base_dir.join(space).join(label);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries: Vec<MigrationFileEntry> = Vec::new();
        for entry in std::fs::read_dir(&dir)
            .map_err(|e| MigrationError::Plan(format!("failed to read migration dir: {e}")))?
        {
            let entry =
                entry.map_err(|e| MigrationError::Plan(format!("failed to read dir entry: {e}")))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(plan) = serde_json::from_str::<MigrationPlan>(&content) {
                        let seq = extract_sequence(&path);
                        entries.push(MigrationFileEntry {
                            id: format!("V{seq:03}"),
                            space: plan.target.space.clone(),
                            label: plan.target.label.clone(),
                            is_edge: plan.target.is_edge,
                            from_version: plan.version_range.from,
                            to_version: plan.version_range.to,
                            plan_hash: plan.plan_hash.clone(),
                            file_path: path.clone(),
                            applied_at: None,
                            checksum: plan.plan_hash.clone(),
                        });
                    }
                }
            }
        }

        entries.sort_by_key(|e| extract_sequence(&e.file_path));
        Ok(entries)
    }

    /// Get next sequence number for a label.
    pub fn next_sequence(&self, space: &str, label: &str) -> Result<u32, MigrationError> {
        let files = self.list_files(space, label, false)?;
        let max_seq = files
            .iter()
            .map(|e| extract_sequence(&e.file_path))
            .max()
            .unwrap_or(0);
        Ok(max_seq + 1)
    }

    /// Load a migration plan from a file path.
    pub fn load_plan(&self, path: &Path) -> Result<MigrationPlan, MigrationError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| MigrationError::Plan(format!("failed to read migration file: {e}")))?;
        serde_json::from_str::<MigrationPlan>(&content)
            .map_err(|e| MigrationError::Plan(format!("failed to parse migration file: {e}")))
    }

    /// Delete a migration file.
    pub fn delete_file(&self, entry: &MigrationFileEntry) -> Result<(), MigrationError> {
        if entry.file_path.exists() {
            std::fs::remove_file(&entry.file_path)
                .map_err(|e| MigrationError::Plan(format!("failed to delete migration file: {e}")))?;
        }
        Ok(())
    }
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
        .to_lowercase()
}

fn extract_sequence(path: &Path) -> u32 {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix('V'))
        .and_then(|s| s.split("__").next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{MigrationTarget, SafetyLevel, VersionRange};

    fn make_plan(space: &str, label: &str) -> MigrationPlan {
        MigrationPlan::new(
            MigrationTarget {
                space: space.to_string(),
                label: label.to_string(),
                is_edge: false,
            },
            VersionRange { from: 1, to: 2 },
            vec![],
            0,
            SafetyLevel::Safe,
            None,
        )
    }

    #[test]
    fn test_registry_create_and_list() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = MigrationFileRegistry::new(tmp.path().to_path_buf());
        let plan = make_plan("space1", "label1");
        let seq = registry.next_sequence("space1", "label1").unwrap();
        assert_eq!(seq, 1);
        let path = registry.create_migration_file(&plan, seq).unwrap();
        assert!(path.exists());
        let files = registry.list_files("space1", "label1", false).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, "V001");
        let next = registry.next_sequence("space1", "label1").unwrap();
        assert_eq!(next, 2);
    }

    #[test]
    fn test_sanitize_and_extract() {
        assert_eq!(sanitize_name("Add column 'email'"), "add_column_email");
        let p = PathBuf::from("/tmp/V012__add_email.json");
        assert_eq!(extract_sequence(&p), 12);
        let p2 = PathBuf::from("/tmp/bad.json");
        assert_eq!(extract_sequence(&p2), 0);
    }

    #[test]
    fn test_load_plan_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = MigrationFileRegistry::new(tmp.path().to_path_buf());
        let plan = make_plan("s", "l");
        let path = registry.create_migration_file(&plan, 1).unwrap();
        let loaded = registry.load_plan(&path).unwrap();
        assert_eq!(loaded.target.space, "s");
        assert_eq!(loaded.plan_hash, plan.plan_hash);
    }

    #[test]
    fn test_list_nonexistent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = MigrationFileRegistry::new(tmp.path().to_path_buf());
        let files = registry.list_files("nonexist", "label", false).unwrap();
        assert!(files.is_empty());
    }
}
