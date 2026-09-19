//! Orphan group-file garbage collection after checkpoints.

use super::super::core::EdgeStore;
use super::layout::parse_group_file;
use std::collections::HashSet;
use std::path::Path;

impl EdgeStore {
    pub(crate) fn remove_orphan_group_files(&self, dir: &Path) {
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
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let path = entry.path();
                if name.starts_with("out_g") && name.ends_with(".bin") && !name.contains(".append")
                {
                    if let Some(gid) = parse_group_file(&name, "out_g") {
                        if !out_existing.contains(&gid) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                } else if name.starts_with("in_g")
                    && name.ends_with(".bin")
                    && !name.contains(".append")
                {
                    if let Some(gid) = parse_group_file(&name, "in_g") {
                        if !in_existing.contains(&gid) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                } else if name.starts_with("out_g") && name.contains(".append") {
                    if let Some(gid) = parse_group_file(&name, "out_g") {
                        if !out_existing.contains(&gid) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                } else if name.starts_with("in_g") && name.contains(".append") {
                    if let Some(gid) = parse_group_file(&name, "in_g") {
                        if !in_existing.contains(&gid) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                } else if name.ends_with(".serving") {
                    // Serving sidecars track their base group: drop the cache
                    // when the group itself is gone.
                    let base = name.strip_suffix(".serving").unwrap_or("");
                    if base.starts_with("out_g") {
                        if let Some(gid) = parse_group_file(base, "out_g") {
                            if !out_existing.contains(&gid) {
                                let _ = std::fs::remove_file(&path);
                            }
                        }
                    } else if base.starts_with("in_g") {
                        if let Some(gid) = parse_group_file(base, "in_g") {
                            if !in_existing.contains(&gid) {
                                let _ = std::fs::remove_file(&path);
                            }
                        }
                    }
                } else if name.starts_with("ts_g") && name.ends_with(".bin") {
                    if let Some(gid) = parse_group_file(&name, "ts_g") {
                        if !owner_existing.contains(&gid) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                } else if name.starts_with("props_g") && name.ends_with(".bin") {
                    if let Some(gid) = parse_group_file(&name, "props_g") {
                        if !owner_existing.contains(&gid) {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                } else if name == super::layout::LEGACY_PROPERTIES_FILE {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
}
