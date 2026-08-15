//! Cold snapshot DTOs mirrored from the server's snapshot endpoints.

use serde::Deserialize;

/// Metadata describing one registered cold snapshot.
#[derive(Debug, Clone, Deserialize)]
pub struct ColdSnapshotInfo {
    pub label: u32,
    #[serde(default)]
    pub label_name: String,
    #[serde(default)]
    pub snapshot_ts: u64,
    #[serde(default)]
    pub edge_count: u64,
    #[serde(default)]
    pub file_path: String,
    #[serde(default)]
    pub file_size: u64,
    #[serde(default)]
    pub checksum: u32,
}
