//! Basic types in the graph space

use crate::core::types::{DataType, EdgeTypeInfo, MetadataVersion, TagInfo};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Charset and collation information
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharsetInfo {
    pub charset: String,
    pub collation: String,
}

/// Isolation level for space storage
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum IsolationLevel {
    /// Shared storage (default) - all spaces share the same base path
    #[default]
    Shared,
    /// Independent subdirectory - each space has its own subdirectory
    Directory,
    /// Independent storage device - each space can have a custom storage path
    Device,
}

/// Space status for lifecycle management
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SpaceStatus {
    /// Space is online and fully operational
    #[default]
    Online,
    /// Space is offline and cannot be accessed
    Offline,
    /// Space is in maintenance mode - read-only operations allowed
    Maintenance,
    /// Space is in read-only mode
    ReadOnly,
}

impl SpaceStatus {
    pub fn is_writable(&self) -> bool {
        matches!(self, Self::Online)
    }

    pub fn is_accessible(&self) -> bool {
        matches!(self, Self::Online | Self::Maintenance | Self::ReadOnly)
    }
}

/// Storage engine type for space
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum EngineType {
    #[default]
    Redb,
    Memory,
}

/// Lightweight space information for session context
///
/// This is a simplified version of SpaceInfo used in API and session layers.
/// It contains only the essential information needed for query execution.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpaceSummary {
    pub id: u64,
    pub name: String,
    pub vid_type: DataType,
    pub status: SpaceStatus,
}

impl SpaceSummary {
    pub fn new(id: u64, name: String, vid_type: DataType) -> Self {
        Self {
            id,
            name,
            vid_type,
            status: SpaceStatus::Online,
        }
    }

    pub fn with_status(mut self, status: SpaceStatus) -> Self {
        self.status = status;
        self
    }

    pub fn is_writable(&self) -> bool {
        self.status.is_writable()
    }

    pub fn is_accessible(&self) -> bool {
        self.status.is_accessible()
    }
}

impl From<SpaceInfo> for SpaceSummary {
    fn from(info: SpaceInfo) -> Self {
        Self {
            id: info.space_id,
            name: info.space_name,
            vid_type: info.vid_type,
            status: info.status,
        }
    }
}

impl From<&SpaceInfo> for SpaceSummary {
    fn from(info: &SpaceInfo) -> Self {
        Self {
            id: info.space_id,
            name: info.space_name.clone(),
            vid_type: info.vid_type.clone(),
            status: info.status.clone(),
        }
    }
}

impl From<SpaceSummary> for SpaceInfo {
    fn from(summary: SpaceSummary) -> Self {
        Self {
            space_id: summary.id,
            space_name: summary.name,
            vid_type: summary.vid_type,
            status: summary.status,
            tags: Vec::new(),
            edge_types: Vec::new(),
            version: MetadataVersion::default(),
            comment: None,
            storage_path: None,
            isolation_level: IsolationLevel::default(),
            partition_num: 100,
            replica_factor: 1,
            engine_type: EngineType::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpaceInfo {
    pub space_id: u64,
    pub space_name: String,
    pub vid_type: DataType,
    pub tags: Vec<TagInfo>,
    pub edge_types: Vec<EdgeTypeInfo>,
    pub version: MetadataVersion,
    pub comment: Option<String>,
    /// Custom storage path for this space (optional)
    pub storage_path: Option<PathBuf>,
    /// Isolation level for storage
    pub isolation_level: IsolationLevel,
    /// Number of partitions (default: 100)
    pub partition_num: i32,
    /// Replica factor (fixed to 1 for single-node deployment)
    pub replica_factor: i32,
    /// Storage engine type
    pub engine_type: EngineType,
    /// Space status for lifecycle management
    pub status: SpaceStatus,
}

impl SpaceInfo {
    pub fn new(space_name: String) -> Self {
        Self {
            space_id: 0,
            space_name,
            vid_type: DataType::String,
            tags: Vec::new(),
            edge_types: Vec::new(),
            version: MetadataVersion::default(),
            comment: None,
            storage_path: None,
            isolation_level: IsolationLevel::default(),
            partition_num: 100,
            replica_factor: 1,
            engine_type: EngineType::default(),
            status: SpaceStatus::Online,
        }
    }

    pub fn with_id(mut self, id: u64) -> Self {
        self.space_id = id;
        self
    }

    pub fn with_vid_type(mut self, vid_type: DataType) -> Self {
        self.vid_type = vid_type;
        self
    }

    pub fn with_comment(mut self, comment: Option<String>) -> Self {
        self.comment = comment;
        self
    }

    pub fn with_storage_path(mut self, storage_path: Option<PathBuf>) -> Self {
        self.storage_path = storage_path;
        if self.storage_path.is_some() {
            self.isolation_level = IsolationLevel::Device;
        }
        self
    }

    pub fn with_isolation_level(mut self, isolation_level: IsolationLevel) -> Self {
        self.isolation_level = isolation_level;
        self
    }

    pub fn with_partition_num(mut self, partition_num: i32) -> Self {
        self.partition_num = partition_num;
        self
    }

    pub fn with_replica_factor(mut self, replica_factor: i32) -> Self {
        self.replica_factor = replica_factor;
        self
    }

    pub fn with_engine_type(mut self, engine_type: EngineType) -> Self {
        self.engine_type = engine_type;
        self
    }

    pub fn with_status(mut self, status: SpaceStatus) -> Self {
        self.status = status;
        self
    }

    pub fn summary(&self) -> SpaceSummary {
        SpaceSummary::from(self)
    }

    pub fn is_writable(&self) -> bool {
        self.status.is_writable()
    }

    pub fn is_accessible(&self) -> bool {
        self.status.is_accessible()
    }
}

impl Default for SpaceInfo {
    fn default() -> Self {
        Self::new("default".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_space_status() {
        assert!(SpaceStatus::Online.is_writable());
        assert!(SpaceStatus::Online.is_accessible());

        assert!(!SpaceStatus::Offline.is_writable());
        assert!(!SpaceStatus::Offline.is_accessible());

        assert!(!SpaceStatus::Maintenance.is_writable());
        assert!(SpaceStatus::Maintenance.is_accessible());

        assert!(!SpaceStatus::ReadOnly.is_writable());
        assert!(SpaceStatus::ReadOnly.is_accessible());
    }

    #[test]
    fn test_space_summary_status() {
        let summary = SpaceSummary::new(1, "test".to_string(), DataType::String);
        assert!(summary.is_writable());
        assert!(summary.is_accessible());

        let offline_summary = summary.clone().with_status(SpaceStatus::Offline);
        assert!(!offline_summary.is_writable());
        assert!(!offline_summary.is_accessible());
    }

    #[test]
    fn test_space_info_builder() {
        let info = SpaceInfo::new("test_space".to_string())
            .with_id(1)
            .with_partition_num(200)
            .with_replica_factor(1)
            .with_engine_type(EngineType::Redb)
            .with_status(SpaceStatus::Online);

        assert_eq!(info.space_id, 1);
        assert_eq!(info.partition_num, 200);
        assert_eq!(info.replica_factor, 1);
        assert!(info.is_writable());
    }
}
