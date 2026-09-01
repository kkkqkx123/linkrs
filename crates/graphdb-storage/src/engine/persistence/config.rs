use std::path::{Path, PathBuf};
use std::time::Duration;

use graphdb_transaction::wal::SyncPolicy;

use crate::engine::config::PropertyGraphConfig;

#[derive(Debug, Clone)]
pub struct PersistenceConfig {
    pub data_dir: PathBuf,
    pub wal_dir: PathBuf,
    pub checkpoint_dir: PathBuf,
    pub snapshot_dir: PathBuf,
    pub auto_flush_interval: Duration,
    pub auto_checkpoint_interval: Duration,
    pub checkpoint_threshold: u64,
    pub max_wal_size: u64,
    pub enable_snapshots: bool,
    pub snapshot_interval: Duration,
    /// Should WAL be enabled
    pub enable_wal: bool,
    /// Synchronization policy for WAL write-ahead logging
    pub sync_policy: Option<SyncPolicy>,
    /// Property graph resource and maintenance configuration.
    pub property_graph_config: PropertyGraphConfig,
    /// Whether async background checkpoint scheduling is enabled.
    pub async_checkpoint_enabled: bool,
    /// Interval for background checkpoint polling.
    pub async_checkpoint_poll_interval: Duration,
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            data_dir: PathBuf::from("data"),
            wal_dir: PathBuf::from("wal"),
            checkpoint_dir: PathBuf::from("checkpoint"),
            snapshot_dir: PathBuf::from("snapshots"),
            auto_flush_interval: Duration::from_secs(60),
            auto_checkpoint_interval: Duration::from_secs(300),
            checkpoint_threshold: 10000,
            max_wal_size: 100 * 1024 * 1024,
            enable_snapshots: true,
            snapshot_interval: Duration::from_secs(3600),
            enable_wal: true,
            sync_policy: Some(SyncPolicy::EveryWrite),
            property_graph_config: PropertyGraphConfig::default(),
            async_checkpoint_enabled: true,
            async_checkpoint_poll_interval: Duration::from_secs(1),
        }
    }
}

impl PersistenceConfig {
    pub fn for_work_dir(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        Self {
            data_dir: path.join("data"),
            wal_dir: path.join("wal"),
            checkpoint_dir: path.join("checkpoint"),
            snapshot_dir: path.join("snapshots"),
            enable_wal: true,
            sync_policy: Some(SyncPolicy::EveryWrite),
            property_graph_config: PropertyGraphConfig::default(),
            ..Default::default()
        }
    }

    pub fn with_property_graph_config(mut self, config: PropertyGraphConfig) -> Self {
        self.property_graph_config = config;
        self
    }
}
