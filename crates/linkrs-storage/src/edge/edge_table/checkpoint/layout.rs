//! Directory layout for one edge-table checkpoint directory.

use super::super::core::EdgeStore;
use crate::edge::node_group::TableShardManifest;
use std::path::{Path, PathBuf};

/// Manifest file inside an edge-table directory.
pub const GROUPS_MANIFEST_FILE: &str = "groups_manifest.bin";
/// Segment-statistics snapshot inside an edge-table directory.
pub const SEGMENT_STATS_FILE: &str = "segment_stats.bin";
/// Width and access profile snapshot inside an edge-table directory.
pub const FORM_PROFILE_FILE: &str = "form_profile.bin";

pub fn out_group_file(group: usize) -> String {
    format!("out_g{}.bin", group)
}

pub fn in_group_file(group: usize) -> String {
    format!("in_g{}.bin", group)
}

pub fn out_append_file(group: usize) -> String {
    format!("out_g{}.append.bin", group)
}

pub fn in_append_file(group: usize) -> String {
    format!("in_g{}.append.bin", group)
}

pub fn ts_group_file(group: u32) -> String {
    format!("ts_g{}.bin", group)
}

pub fn props_group_file(group: u32) -> String {
    format!("props_g{}.bin", group)
}

pub(crate) fn out_group_path(dir: &Path, group: usize) -> PathBuf {
    dir.join(out_group_file(group))
}

pub(crate) fn in_group_path(dir: &Path, group: usize) -> PathBuf {
    dir.join(in_group_file(group))
}

pub(crate) fn out_append_path(dir: &Path, group: usize) -> PathBuf {
    dir.join(out_append_file(group))
}

pub(crate) fn in_append_path(dir: &Path, group: usize) -> PathBuf {
    dir.join(in_append_file(group))
}

pub(crate) fn ts_group_path(dir: &Path, group: u32) -> PathBuf {
    dir.join(ts_group_file(group))
}

pub(crate) fn props_group_path(dir: &Path, group: u32) -> PathBuf {
    dir.join(props_group_file(group))
}

pub(crate) fn manifest_path(dir: &Path) -> PathBuf {
    dir.join(GROUPS_MANIFEST_FILE)
}

pub(crate) fn segment_stats_path(dir: &Path) -> PathBuf {
    dir.join(SEGMENT_STATS_FILE)
}

pub(crate) fn form_profile_path(dir: &Path) -> PathBuf {
    dir.join(FORM_PROFILE_FILE)
}

/// Parse `"<prefix>{gid}.bin"` or `"<prefix>{gid}.append.bin"` into the gid.
/// Returns `None` for foreign files so orphan cleanup never deletes them.
/// The match is exact: a shared numeric prefix with a foreign suffix (for
/// example `out_g3_evil.bin`) is third-party and skipped.
pub(crate) fn parse_group_file(name: &str, prefix: &str) -> Option<u32> {
    let rest = name.strip_prefix(prefix)?;
    for suffix in [".append.bin", ".bin"] {
        if let Some(digits) = rest.strip_suffix(suffix) {
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                return digits.parse::<u32>().ok();
            }
            return None;
        }
    }
    None
}

/// File size for checkpoint byte accounting. Metrics must never fail a
/// checkpoint, so a missing file reports zero instead of an error.
pub(crate) fn file_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

impl EdgeStore {
    pub(crate) fn owner_list_for_load(&self, manifest: &TableShardManifest) -> Vec<u32> {
        if self.schema.oe_strategy != crate::edge::EdgeStrategy::None {
            manifest.out_groups.clone()
        } else {
            manifest.in_groups.clone()
        }
    }
}
