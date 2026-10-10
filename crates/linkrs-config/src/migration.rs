use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn default_batch_size() -> usize {
    1000
}

fn default_drain_timeout_ms() -> u64 {
    5000
}

/// Migration execution settings.
///
/// Directory fields left as `None` are derived from the storage data
/// directory (`migration/checkpoints`, `migration/backups`,
/// `migration.lock`); when no data directory is available the
/// corresponding capability stays disabled instead of failing.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MigrationConfig {
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    #[serde(default)]
    pub checkpoint_dir: Option<PathBuf>,
    #[serde(default)]
    pub backup_dir: Option<PathBuf>,
    #[serde(default)]
    pub lock_path: Option<PathBuf>,
    #[serde(default = "default_drain_timeout_ms")]
    pub drain_timeout_ms: u64,
    #[serde(default)]
    pub min_free_bytes: u64,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            batch_size: default_batch_size(),
            checkpoint_dir: None,
            backup_dir: None,
            lock_path: None,
            drain_timeout_ms: default_drain_timeout_ms(),
            min_free_bytes: 0,
        }
    }
}

impl MigrationConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.batch_size == 0 {
            return Err("Migration batch_size must be greater than 0".to_string());
        }
        if self.drain_timeout_ms == 0 {
            return Err("Migration drain_timeout_ms must be greater than 0".to_string());
        }
        Ok(())
    }

    /// Resolve an optional directory against the storage data directory.
    /// Explicit values win; otherwise derive `migration/<sub>` under the
    /// data directory. Empty data directories leave the capability off.
    fn resolve_dir(&self, explicit: Option<PathBuf>, sub: &str, data_dir: &str) -> Option<PathBuf> {
        if let Some(path) = explicit {
            return Some(path);
        }
        if data_dir.is_empty() {
            return None;
        }
        Some(Path::new(data_dir).join("migration").join(sub))
    }

    pub fn resolved_checkpoint_dir(&self, data_dir: &str) -> Option<PathBuf> {
        self.resolve_dir(self.checkpoint_dir.clone(), "checkpoints", data_dir)
    }

    pub fn resolved_backup_dir(&self, data_dir: &str) -> Option<PathBuf> {
        self.resolve_dir(self.backup_dir.clone(), "backups", data_dir)
    }

    pub fn resolved_lock_path(&self, data_dir: &str) -> Option<PathBuf> {
        if let Some(path) = self.lock_path.clone() {
            return Some(path);
        }
        if data_dir.is_empty() {
            return None;
        }
        Some(Path::new(data_dir).join("migration.lock"))
    }

    pub(crate) fn resolve_relative_paths(
        &mut self,
        base_dir: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.checkpoint_dir = resolve_optional(base_dir, self.checkpoint_dir.take())?;
        self.backup_dir = resolve_optional(base_dir, self.backup_dir.take())?;
        self.lock_path = resolve_optional(base_dir, self.lock_path.take())?;
        Ok(())
    }
}

fn resolve_optional(
    base_dir: &Path,
    value: Option<PathBuf>,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    value
        .map(|path| {
            if path.is_absolute() {
                Ok(path)
            } else {
                Ok(base_dir.join(path))
            }
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migration_config_defaults() {
        let config = MigrationConfig::default();
        assert_eq!(config.batch_size, 1000);
        assert_eq!(config.drain_timeout_ms, 5000);
        assert_eq!(config.min_free_bytes, 0);
        assert!(config.checkpoint_dir.is_none());
        assert!(config.backup_dir.is_none());
        assert!(config.lock_path.is_none());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_migration_config_validate_rejects_zero() {
        let invalid = MigrationConfig {
            batch_size: 0,
            ..Default::default()
        };
        assert!(invalid.validate().is_err());
        let invalid = MigrationConfig {
            drain_timeout_ms: 0,
            ..Default::default()
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn test_resolved_dirs_derive_from_data_dir() {
        let config = MigrationConfig::default();
        let data_dir = "/tmp/linkrs";
        assert_eq!(
            config.resolved_checkpoint_dir(data_dir),
            Some(PathBuf::from("/tmp/linkrs/migration/checkpoints"))
        );
        assert_eq!(
            config.resolved_backup_dir(data_dir),
            Some(PathBuf::from("/tmp/linkrs/migration/backups"))
        );
        assert_eq!(
            config.resolved_lock_path(data_dir),
            Some(PathBuf::from("/tmp/linkrs/migration.lock"))
        );
    }

    #[test]
    fn test_resolved_dirs_empty_data_dir_disables() {
        let config = MigrationConfig::default();
        assert!(config.resolved_checkpoint_dir("").is_none());
        assert!(config.resolved_backup_dir("").is_none());
        assert!(config.resolved_lock_path("").is_none());
    }

    #[test]
    fn test_resolved_dirs_explicit_wins() {
        let config = MigrationConfig {
            checkpoint_dir: Some(PathBuf::from("/explicit/cp")),
            backup_dir: Some(PathBuf::from("/explicit/bk")),
            lock_path: Some(PathBuf::from("/explicit.lock")),
            ..Default::default()
        };
        let data_dir = "/tmp/linkrs";
        assert_eq!(
            config.resolved_checkpoint_dir(data_dir),
            Some(PathBuf::from("/explicit/cp"))
        );
        assert_eq!(
            config.resolved_backup_dir(data_dir),
            Some(PathBuf::from("/explicit/bk"))
        );
        assert_eq!(
            config.resolved_lock_path(data_dir),
            Some(PathBuf::from("/explicit.lock"))
        );
    }
}
