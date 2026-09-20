//! Orphan group-file garbage collection after checkpoints.
//!
//! File-level cleanup only: removes group files whose group no longer exists.
//! Data-level orphan convergence (authority or property rows without topology)
//! lives in `core::owner::rebuild_owner_map_with_stats` and the load-time
//! `copy_audit` refusal. The two must never be confused: this module counts
//! deleted files, the audit counts converged or refused data rows.

use super::super::core::EdgeStore;
use super::layout::parse_group_file;
use std::collections::HashSet;
use std::path::Path;

impl EdgeStore {
    /// Remove group files whose group no longer exists, returning the removed
    /// count for observability.
    ///
    /// Timestamp and property shards follow the owner direction's groups, so
    /// orphan shard convergence is visible here as a count instead of silent
    /// deletion. Serving sidecars track their base group and drop with it.
    pub(crate) fn remove_orphan_group_files(&self, dir: &Path) -> usize {
        let out_existing: HashSet<u32> = self
            .out_csr
            .existing_group_ids()
            .into_iter()
            .map(|gid| gid as u32)
            .collect();
        let in_existing: HashSet<u32> = self
            .in_csr
            .existing_group_ids()
            .into_iter()
            .map(|gid| gid as u32)
            .collect();
        let owner_existing: HashSet<u32> =
            if self.schema.oe_strategy != crate::edge::EdgeStrategy::None {
                out_existing.clone()
            } else {
                in_existing.clone()
            };
        let mut removed = 0usize;
        let mut remove = |path: &std::path::Path| {
            if std::fs::remove_file(path).is_ok() {
                removed += 1;
            }
        };
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();
                if name.starts_with("out_g") && name.ends_with(".bin") && !name.contains(".append")
                {
                    if let Some(gid) = parse_group_file(&name, "out_g") {
                        if !out_existing.contains(&gid) {
                            remove(&path);
                        }
                    }
                } else if name.starts_with("in_g")
                    && name.ends_with(".bin")
                    && !name.contains(".append")
                {
                    if let Some(gid) = parse_group_file(&name, "in_g") {
                        if !in_existing.contains(&gid) {
                            remove(&path);
                        }
                    }
                } else if name.starts_with("out_g") && name.contains(".append") {
                    if let Some(gid) = parse_group_file(&name, "out_g") {
                        if !out_existing.contains(&gid) {
                            remove(&path);
                        }
                    }
                } else if name.starts_with("in_g") && name.contains(".append") {
                    if let Some(gid) = parse_group_file(&name, "in_g") {
                        if !in_existing.contains(&gid) {
                            remove(&path);
                        }
                    }
                } else if name.ends_with(".serving") {
                    // Serving sidecars track their base group: drop the cache
                    // when the group itself is gone.
                    let base = name.strip_suffix(".serving").unwrap_or("");
                    if base.starts_with("out_g") {
                        if let Some(gid) = parse_group_file(base, "out_g") {
                            if !out_existing.contains(&gid) {
                                remove(&path);
                            }
                        }
                    } else if base.starts_with("in_g") {
                        if let Some(gid) = parse_group_file(base, "in_g") {
                            if !in_existing.contains(&gid) {
                                remove(&path);
                            }
                        }
                    }
                } else if name.starts_with("ts_g") && name.ends_with(".bin") {
                    if let Some(gid) = parse_group_file(&name, "ts_g") {
                        if !owner_existing.contains(&gid) {
                            remove(&path);
                        }
                    }
                } else if name.starts_with("props_g") && name.ends_with(".bin") {
                    if let Some(gid) = parse_group_file(&name, "props_g") {
                        if !owner_existing.contains(&gid) {
                            remove(&path);
                        }
                    }
                }
            }
        }
        if removed > 0 {
            log::debug!("remove_orphan_group_files: removed {} orphan files", removed);
        }
        removed
    }
}
