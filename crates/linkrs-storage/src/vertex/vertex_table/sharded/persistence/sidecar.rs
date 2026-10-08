//! Derived snapshot sidecars: verifiable but discardable eviction caches.

use std::path::Path;

use super::super::ShardedVertexTable;

/// One checkpoint sidecar pinned in the commit manifest: a derived
/// `{column}.snapshot` eviction cache beside the authoritative pages.
/// Sidecars are verifiable but discardable: a missing or corrupt sidecar
/// keeps chunks resident and never refuses the open, while the manifest
/// pin lets reload tell a pruned sidecar from a tampered one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SnapshotSidecarRecord {
    /// Manifest-relative path (`shard_0/name.snapshot`).
    pub(crate) file: String,
    /// File bytes at flush time.
    pub(crate) bytes: u64,
    /// CRC32 over the file bytes at flush time.
    pub(crate) checksum: u32,
}

/// CRC32 over one sidecar file's bytes, with its length. `None` when the
/// file cannot be read; the caller records no pin instead of failing the
/// checkpoint over a derived cache.
pub(crate) fn sidecar_file_fingerprint(path: &Path) -> Option<(u64, u32)> {
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&bytes);
    Some((bytes.len() as u64, hasher.finalize()))
}

/// Inventory every `{column}.snapshot` sidecar under the table directory
/// as manifest-relative records. Runs after the per-shard sidecar flush
/// and before the commit manifest write, so the manifest always points at
/// sidecars that exist; post-manifest orphan sweep drops only unpinned
/// sidecars. Unreadable sidecars are skipped (derived cache, never fatal).
pub(crate) fn collect_sidecar_records(dir: &Path) -> Vec<SnapshotSidecarRecord> {
    let mut out = Vec::new();
    for index in 0..usize::MAX {
        let shard_dir = dir.join(format!("shard_{}", index));
        if !shard_dir.exists() {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&shard_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !name.ends_with(".snapshot") {
                continue;
            }
            let Some((bytes, checksum)) = sidecar_file_fingerprint(&path) else {
                continue;
            };
            out.push(SnapshotSidecarRecord {
                file: format!("shard_{}/{}", index, name),
                bytes,
                checksum,
            });
        }
    }
    out.sort_by(|a, b| a.file.cmp(&b.file));
    out
}

/// Remove sidecar files that the committed manifest does not pin. Runs
/// after the manifest commit so a crash between sidecar flush and manifest
/// write never deletes a sidecar the previous manifest still pins; only
/// sidecars absent from the new manifest (dropped columns, fully resident
/// tables) are swept. Tolerant: delete failures only warn.
pub(crate) fn sweep_unpinned_sidecars(dir: &Path, pinned: &[SnapshotSidecarRecord]) {
    use std::collections::HashSet;
    let live: HashSet<&str> = pinned.iter().map(|r| r.file.as_str()).collect();
    for index in 0..usize::MAX {
        let shard_dir = dir.join(format!("shard_{}", index));
        if !shard_dir.exists() {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&shard_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if !name.ends_with(".snapshot") {
                continue;
            }
            let rel = format!("shard_{}/{}", index, name);
            if live.contains(rel.as_str()) {
                continue;
            }
            if let Err(e) = std::fs::remove_file(&path) {
                log::warn!(
                    "orphan sidecar cleanup: cannot remove {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }
}

/// Verify pinned sidecars against disk and prune tampered ones before
/// any shard loads. A missing sidecar needs no action (kept resident);
/// a present sidecar whose bytes or checksum differ from the pin, or
/// whose frames fail to parse, is deleted as a discardable cache and
/// counted. Only `.snapshot` files are ever removed, never
/// authoritative pages, and failures only warn. Returns pruned count.
pub(crate) fn prune_tampered_sidecars(dir: &Path, sidecars: &[SnapshotSidecarRecord]) -> usize {
    let mut pruned = 0usize;
    for record in sidecars {
        let full = dir.join(&record.file);
        if !full.exists() {
            continue;
        }
        let valid = match std::fs::read(&full) {
            Ok(payload) => {
                let mut hasher = crc32fast::Hasher::new();
                hasher.update(&payload);
                payload.len() as u64 == record.bytes
                    && hasher.finalize() == record.checksum
                    && crate::vertex::column::chunk_residency::open_snapshot_sidecar(&full).is_ok()
            }
            Err(_) => false,
        };
        if !valid {
            log::warn!(
                "pruning tampered snapshot sidecar {}: discarding cache, keeping chunks resident",
                full.display(),
            );
            if std::fs::remove_file(&full).is_ok() {
                pruned += 1;
            } else {
                log::warn!("cannot prune tampered snapshot sidecar {}", full.display(),);
            }
        }
    }
    pruned
}

impl ShardedVertexTable {
    /// Test-only force-encode plus evict for one column across shards.
    /// Uses per-column shared guards (released between columns) so no
    /// mapped guard outlives its shard latch.
    #[cfg(test)]
    pub(crate) fn force_encode_evict_for_test(&self, column: &str) {
        for shard in &self.shards {
            let table = shard.read();
            table.columns.for_each_column(|col| {
                if col.name == column {
                    let _ = col
                        .apply_encoding_to_chunks(crate::encoding::EncodingType::Dictionary, 255);
                    let _ = col.evict_chunk(0);
                }
            });
        }
    }
}
