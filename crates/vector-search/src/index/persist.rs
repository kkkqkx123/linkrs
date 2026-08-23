//! `index.bin` — persisted IVFFlat state (centroids + slot membership).
//!
//! The IVF index is a *derived* structure: it can be rebuilt from
//! `vectors.bin` at any time, so persistence is a pure optimization for open
//! latency. Any validation failure on load discards the file and falls back to
//! exact scan rather than blocking startup.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Result, VectorSearchError};
use crate::types::DistanceMetric;

pub(crate) const INDEX_MAGIC: [u8; 4] = *b"VIVF";
/// v2: added `baseline_mean_dist` (drift baseline).
pub(crate) const INDEX_VERSION: u16 = 2;
const INDEX_TMP: &str = "index_tmp.bin";

/// The persisted subset of the IVF index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PersistedIvf {
    pub lists: u32,
    pub dim: usize,
    pub distance: DistanceMetric,
    pub built_at_live_count: u64,
    /// Mean training-sample distance to the nearest centroid; drift baseline.
    pub baseline_mean_dist: f32,
    /// `lists x dim` centroids in list order.
    pub centroids: Vec<Vec<f32>>,
    /// slot -> list assignment; `u32::MAX` = unassigned. May be shorter than
    /// the collection's slot high-water mark (slots appended after the build
    /// are adopted through the pending path on publish/replay).
    pub slot_list: Vec<u32>,
}

impl PersistedIvf {
    /// Structural self-check independent of collection state.
    pub(crate) fn structurally_valid(&self) -> bool {
        self.lists > 0
            && self.dim > 0
            && self.baseline_mean_dist.is_finite()
            && self.baseline_mean_dist >= 0.0
            && self.centroids.len() == self.lists as usize
            && self.centroids.iter().all(|c| c.len() == self.dim)
            && self
                .slot_list
                .iter()
                .all(|&l| l == u32::MAX || (l as usize) < self.centroids.len())
    }

    /// Check against the live collection metadata on open.
    pub(crate) fn valid_for(&self, dim: usize, distance: DistanceMetric, next_slot: u64) -> bool {
        self.structurally_valid()
            && self.dim == dim
            && self.distance == distance
            && self.slot_list.len() as u64 <= next_slot
    }
}

/// Write `data` to `<dir>/index.bin` atomically (temp file + fsync + rename).
pub(crate) fn save(dir: &Path, data: &PersistedIvf) -> Result<()> {
    let bytes = postcard::to_stdvec(data)?;
    let tmp = dir.join(INDEX_TMP);
    {
        let mut file = File::create(&tmp)?;
        file.write_all(&INDEX_MAGIC)?;
        file.write_all(&INDEX_VERSION.to_le_bytes())?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, dir.join("index.bin"))?;
    Ok(())
}

/// Load `<dir>/index.bin`. Returns `Ok(None)` when the file is absent or
/// fails any structural check; a failing check also deletes the file so the
/// next save starts clean.
pub(crate) fn load(dir: &Path) -> Result<Option<PersistedIvf>> {
    let path = dir.join("index.bin");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let parsed = (|| -> Result<PersistedIvf> {
        if bytes.len() < 6 || bytes[..4] != INDEX_MAGIC {
            return Err(VectorSearchError::CorruptData(
                "index.bin bad magic".to_string(),
            ));
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != INDEX_VERSION {
            return Err(VectorSearchError::CorruptData(format!(
                "index.bin unsupported version {version}"
            )));
        }
        Ok(postcard::from_bytes(&bytes[6..])?)
    })();

    match parsed {
        // Any malformed content is treated as absent: the index is a derived
        // structure and must never block startup.
        Ok(data) if data.structurally_valid() => Ok(Some(data)),
        _ => {
            discard(&path);
            Ok(None)
        }
    }
}

pub(crate) fn discard(path: &Path) {
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(lists: u32, dim: usize) -> PersistedIvf {
        let centroids = (0..lists).map(|i| vec![i as f32; dim]).collect();
        let slot_list = vec![0u32, 1, u32::MAX, lists - 1];
        PersistedIvf {
            lists,
            dim,
            distance: DistanceMetric::Cosine,
            built_at_live_count: 4,
            baseline_mean_dist: 0.5,
            centroids,
            slot_list,
        }
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let data = sample(3, 4);
        save(dir.path(), &data).unwrap();
        let loaded = load(dir.path()).unwrap().unwrap();
        assert_eq!(loaded.lists, 3);
        assert_eq!(loaded.dim, 4);
        assert_eq!(loaded.slot_list.len(), 4);
        assert_eq!(loaded.built_at_live_count, 4);
        assert_eq!(loaded.baseline_mean_dist, 0.5);
    }

    #[test]
    fn test_load_absent_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).unwrap().is_none());
    }

    #[test]
    fn test_load_corrupt_discards_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.bin"), b"XXXXgarbage").unwrap();
        assert!(load(dir.path()).unwrap().is_none());
        assert!(!dir.path().join("index.bin").exists());
    }

    #[test]
    fn test_load_truncated_postcard_discards_file() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &sample(2, 3)).unwrap();
        let mut bytes = std::fs::read(dir.path().join("index.bin")).unwrap();
        bytes.truncate(10);
        std::fs::write(dir.path().join("index.bin"), &bytes).unwrap();
        assert!(load(dir.path()).unwrap().is_none());
        assert!(!dir.path().join("index.bin").exists());
    }

    #[test]
    fn test_valid_for_checks() {
        let data = sample(2, 3);
        // slot_list covers slots 0..4; valid only if next_slot >= 4.
        assert!(data.valid_for(3, DistanceMetric::Cosine, 4));
        assert!(
            !data.valid_for(4, DistanceMetric::Cosine, 4),
            "dim mismatch"
        );
        assert!(
            !data.valid_for(3, DistanceMetric::Euclid, 4),
            "metric mismatch"
        );
        assert!(
            !data.valid_for(3, DistanceMetric::Cosine, 2),
            "slot_list longer than next_slot"
        );
    }

    #[test]
    fn test_structurally_invalid_rejected_on_save_load() {
        let mut data = sample(2, 3);
        data.centroids[0] = vec![1.0; 9];
        assert!(!data.structurally_valid());
    }
}
