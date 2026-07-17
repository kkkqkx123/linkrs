use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::core::types::{CommitLsn, IndexGeneration, ManifestEpoch, SnapshotTimestamp};
use crate::core::{StorageError, StorageResult};
use crate::storage::cursor::PartitionSelector;

const MANIFEST_FORMAT_VERSION: u16 = 2;

// ── Crash-safe generation rebuild state machine ──

/// Persistent state of a native index generation rebuild.
/// Written to durable storage before each phase so that crash recovery
/// can determine whether the partial build can be resumed or must restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GenerationState {
    /// Building new index data from a fixed MVCC snapshot.
    /// On crash: discard and restart from scratch.
    Building,
    /// Catching up by replaying committed WAL entries since the snapshot LSN.
    /// On crash: discard catch-up progress and restart from the snapshot.
    CatchingUp,
    /// Flushed to checkpoint files; about to atomically publish the manifest.
    /// On crash: publishing must complete before the new generation is usable.
    Publishing,
    /// The new generation is published and active.
    /// On crash: nothing to do.
    Active,
    /// The build failed before publication and must be restarted explicitly.
    Failed,
    /// The build was cancelled before publication and its files may be reclaimed.
    Cancelled,
}

/// Persisted tracking data for one native index generation build.
/// Stored alongside the index metadata so it survives crashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationBuildState {
    /// The new generation number being built.
    pub generation: IndexGeneration,
    /// Manifest epoch reserved for this publication attempt.
    pub manifest_epoch: ManifestEpoch,
    /// MVCC timestamp used by the fixed snapshot scan.
    pub snapshot_timestamp: SnapshotTimestamp,
    /// WAL LSN at which the snapshot was taken. Catch-up replays entries > this.
    pub start_lsn: CommitLsn,
    /// WAL LSN at which the publish fence was established.
    /// All writes with LSN <= barrier_lsn are reflected in the new generation.
    pub barrier_lsn: Option<CommitLsn>,
    /// Current state in the Building → CatchingUp → Publishing → Active sequence.
    pub state: GenerationState,
    /// Durable diagnostic for terminal failure or cancellation.
    pub terminal_reason: Option<String>,
}

impl GenerationBuildState {
    pub fn new(
        generation: IndexGeneration,
        manifest_epoch: ManifestEpoch,
        snapshot_timestamp: SnapshotTimestamp,
        start_lsn: CommitLsn,
    ) -> Self {
        Self {
            generation,
            manifest_epoch,
            snapshot_timestamp,
            start_lsn,
            barrier_lsn: None,
            state: GenerationState::Building,
            terminal_reason: None,
        }
    }

    pub fn transition_to_catching_up(&mut self) -> Result<(), String> {
        self.require_state(GenerationState::Building)?;
        self.state = GenerationState::CatchingUp;
        Ok(())
    }

    pub fn transition_to_publishing(&mut self, barrier_lsn: CommitLsn) -> Result<(), String> {
        self.require_state(GenerationState::CatchingUp)?;
        if barrier_lsn < self.start_lsn {
            return Err("Generation barrier LSN precedes the snapshot LSN".to_string());
        }
        self.barrier_lsn = Some(barrier_lsn);
        self.state = GenerationState::Publishing;
        Ok(())
    }

    /// Bypass the CatchingUp phase (used by split, which operates under an
    /// exclusive fence and needs no incremental replay).
    pub fn transition_from_building_to_publishing(
        &mut self,
        barrier_lsn: CommitLsn,
    ) -> Result<(), String> {
        self.require_state(GenerationState::Building)?;
        self.barrier_lsn = Some(barrier_lsn);
        self.state = GenerationState::Publishing;
        Ok(())
    }

    pub fn transition_to_active(&mut self) -> Result<(), String> {
        self.require_state(GenerationState::Publishing)?;
        self.state = GenerationState::Active;
        Ok(())
    }

    pub fn transition_to_failed(&mut self, reason: impl Into<String>) -> Result<(), String> {
        if matches!(
            self.state,
            GenerationState::Active | GenerationState::Cancelled
        ) {
            return Err(format!("Cannot fail a {:?} generation", self.state));
        }
        self.state = GenerationState::Failed;
        self.terminal_reason = Some(reason.into());
        Ok(())
    }

    pub fn transition_to_cancelled(&mut self, reason: impl Into<String>) -> Result<(), String> {
        if matches!(
            self.state,
            GenerationState::Publishing | GenerationState::Active
        ) {
            return Err(format!("Cannot cancel a {:?} generation", self.state));
        }
        self.state = GenerationState::Cancelled;
        self.terminal_reason = Some(reason.into());
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.state == GenerationState::Active
    }

    pub fn can_resume(&self) -> bool {
        self.state == GenerationState::CatchingUp
    }

    fn require_state(&self, expected: GenerationState) -> Result<(), String> {
        if self.state == expected {
            Ok(())
        } else {
            Err(format!(
                "Invalid generation transition from {:?}; expected {:?}",
                self.state, expected
            ))
        }
    }
}

/// An immutable half-open ordered-key range. `None` represents infinity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexShard {
    pub shard_id: u32,
    pub lower: Option<Vec<u8>>,
    pub upper: Option<Vec<u8>>,
    pub checkpoint_file: PathBuf,
}

impl IndexShard {
    pub fn contains(&self, key: &[u8]) -> bool {
        self.lower.as_deref().is_none_or(|lower| key >= lower)
            && self.upper.as_deref().is_none_or(|upper| key < upper)
    }

    pub fn intersects(&self, lower: Option<&[u8]>, upper: Option<&[u8]>) -> bool {
        self.upper
            .as_deref()
            .zip(lower)
            .is_none_or(|(shard_upper, query_lower)| shard_upper > query_lower)
            && upper
                .zip(self.lower.as_deref())
                .is_none_or(|(query_upper, shard_lower)| query_upper > shard_lower)
    }

    fn validate(&self) -> Result<(), String> {
        if self
            .lower
            .as_ref()
            .zip(self.upper.as_ref())
            .is_some_and(|(lower, upper)| lower >= upper)
        {
            return Err(format!(
                "Index shard {} has an empty or inverted range",
                self.shard_id
            ));
        }
        if self.checkpoint_file.as_os_str().is_empty() {
            return Err(format!(
                "Index shard {} has no checkpoint file",
                self.shard_id
            ));
        }
        Ok(())
    }
}

/// The persisted routing table for one immutable index generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexManifest {
    pub format_version: u16,
    /// Logical namespace of this index. Index IDs are schema-local, so the
    /// pair `(space_id, index_id)` is the physical native-index identity.
    pub space_id: u64,
    pub index_id: u64,
    pub generation: IndexGeneration,
    pub epoch: ManifestEpoch,
    pub shards: Vec<IndexShard>,
}

impl IndexManifest {
    pub fn new(
        space_id: u64,
        index_id: u64,
        generation: IndexGeneration,
        epoch: ManifestEpoch,
        shards: Vec<IndexShard>,
    ) -> Result<Self, String> {
        let manifest = Self {
            format_version: MANIFEST_FORMAT_VERSION,
            space_id,
            index_id,
            generation,
            epoch,
            shards,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format_version != MANIFEST_FORMAT_VERSION {
            return Err(format!(
                "Unsupported index manifest version {}",
                self.format_version
            ));
        }
        if self.shards.is_empty() {
            return Err("Index manifest must contain at least one shard".to_string());
        }
        if self
            .shards
            .first()
            .is_some_and(|shard| shard.lower.is_some())
        {
            return Err("The first index shard must have an unbounded lower range".to_string());
        }
        if self
            .shards
            .last()
            .is_some_and(|shard| shard.upper.is_some())
        {
            return Err("The last index shard must have an unbounded upper range".to_string());
        }
        for shard in &self.shards {
            shard.validate()?;
        }
        for pair in self.shards.windows(2) {
            if pair[0].shard_id == pair[1].shard_id {
                return Err(format!("Duplicate index shard id {}", pair[0].shard_id));
            }
            if pair[0].upper != pair[1].lower {
                return Err(format!(
                    "Index shards {} and {} are not contiguous",
                    pair[0].shard_id, pair[1].shard_id
                ));
            }
        }
        Ok(())
    }

    pub fn route_key(&self, key: &[u8]) -> Option<&IndexShard> {
        self.shards.iter().find(|shard| shard.contains(key))
    }

    pub fn select_shards(&self, selector: &PartitionSelector) -> Vec<&IndexShard> {
        match selector {
            PartitionSelector::All => self.shards.iter().collect(),
            PartitionSelector::Shards(ids) => self
                .shards
                .iter()
                .filter(|shard| ids.contains(&shard.shard_id))
                .collect(),
            PartitionSelector::KeyRange { lower, upper } => self
                .shards
                .iter()
                .filter(|shard| shard.intersects(lower.as_deref(), upper.as_deref()))
                .collect(),
        }
    }

    pub fn scan_ranges(
        &self,
        selector: &PartitionSelector,
        query_lower: &[u8],
        query_upper: &[u8],
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.select_shards(selector)
            .into_iter()
            .filter(|shard| shard.intersects(Some(query_lower), Some(query_upper)))
            .filter_map(|shard| {
                let lower = shard.lower.as_deref().map_or_else(
                    || query_lower.to_vec(),
                    |value| value.max(query_lower).to_vec(),
                );
                let upper = shard.upper.as_deref().map_or_else(
                    || query_upper.to_vec(),
                    |value| value.min(query_upper).to_vec(),
                );
                (lower < upper).then_some((lower, upper))
            })
            .collect()
    }

    pub fn store(&self, path: &Path) -> StorageResult<()> {
        self.validate().map_err(StorageError::db_error)?;
        let parent = path.parent().ok_or_else(|| {
            StorageError::db_error("Index manifest path has no parent".to_string())
        })?;
        std::fs::create_dir_all(parent)?;
        let temporary = path.with_extension("tmp");
        let bytes = serde_json::to_vec(self).map_err(|error| {
            StorageError::db_error(format!("Serialize index manifest: {error}"))
        })?;
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    }

    pub fn load(path: &Path) -> StorageResult<Self> {
        let bytes = std::fs::read(path)?;
        let manifest: Self = serde_json::from_slice(&bytes)
            .map_err(|error| StorageError::db_error(format!("Read index manifest: {error}")))?;
        manifest.validate().map_err(StorageError::db_error)?;
        Ok(manifest)
    }
}

/// A cursor-owned pin that prevents reclamation of its physical generation.
#[derive(Debug, Clone)]
pub struct ManifestHandle(Arc<IndexManifest>);

impl ManifestHandle {
    pub fn manifest(&self) -> &IndexManifest {
        &self.0
    }
}

#[derive(Debug)]
struct RetiredManifest {
    manifest: Arc<IndexManifest>,
}

/// Publishes immutable manifests and fences reclamation with reader handles.
#[derive(Debug)]
pub struct ManifestCatalog {
    active: RwLock<Arc<IndexManifest>>,
    retired: Mutex<Vec<RetiredManifest>>,
    published: AtomicU64,
    reclaimed_files: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestCatalogStats {
    pub active_epoch: ManifestEpoch,
    pub active_generation: IndexGeneration,
    pub active_readers: u64,
    pub retired_generations: u64,
    pub published_manifests: u64,
    pub reclaimed_files: u64,
}

impl ManifestCatalog {
    pub fn new(manifest: IndexManifest) -> Result<Self, String> {
        manifest.validate()?;
        Ok(Self {
            active: RwLock::new(Arc::new(manifest)),
            retired: Mutex::new(Vec::new()),
            published: AtomicU64::new(0),
            reclaimed_files: AtomicU64::new(0),
        })
    }

    pub fn acquire(&self) -> ManifestHandle {
        ManifestHandle(Arc::clone(&self.active.read()))
    }

    pub fn publish(&self, manifest: IndexManifest) -> Result<ManifestHandle, String> {
        manifest.validate()?;
        let mut active = self.active.write();
        if manifest.index_id != active.index_id {
            return Err("Cannot publish a manifest for another index".to_string());
        }
        if manifest.epoch <= active.epoch {
            return Err("Index manifest epoch must increase".to_string());
        }
        if manifest.generation < active.generation {
            return Err("Index generation cannot move backwards".to_string());
        }

        let next = Arc::new(manifest);
        let previous = std::mem::replace(&mut *active, Arc::clone(&next));
        self.retired
            .lock()
            .push(RetiredManifest { manifest: previous });
        self.published.fetch_add(1, Ordering::Relaxed);
        Ok(ManifestHandle(next))
    }

    /// Returns files from generations that have no cursor-owned handles.
    /// The checkpoint owner performs deletion after its own durable fence.
    pub fn take_reclaimable_files(&self) -> Vec<PathBuf> {
        self.take_reclaimable_manifests()
            .into_iter()
            .flat_map(|manifest| {
                manifest
                    .shards
                    .into_iter()
                    .map(|shard| shard.checkpoint_file)
            })
            .collect()
    }

    /// Returns fully retired manifests after their last cursor handle is gone.
    /// Callers use the generation identity to retire matching in-memory data
    /// before deleting the returned checkpoint files.
    pub fn take_reclaimable_manifests(&self) -> Vec<IndexManifest> {
        let mut retired = self.retired.lock();
        let mut manifests = Vec::new();
        retired.retain(|entry| {
            if Arc::strong_count(&entry.manifest) == 1 {
                manifests.push((*entry.manifest).clone());
                false
            } else {
                true
            }
        });
        self.reclaimed_files.fetch_add(
            manifests
                .iter()
                .map(|manifest| manifest.shards.len() as u64)
                .sum(),
            Ordering::Relaxed,
        );
        manifests
    }

    pub fn stats(&self) -> ManifestCatalogStats {
        let active = self.active.read();
        ManifestCatalogStats {
            active_epoch: active.epoch,
            active_generation: active.generation,
            active_readers: Arc::strong_count(&active).saturating_sub(1) as u64,
            retired_generations: self.retired.lock().len() as u64,
            published_manifests: self.published.load(Ordering::Relaxed),
            reclaimed_files: self.reclaimed_files.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{IndexManifest, IndexShard, ManifestCatalog};
    use crate::core::types::{IndexGeneration, ManifestEpoch};
    use crate::storage::cursor::PartitionSelector;

    fn shard(shard_id: u32, lower: Option<&[u8]>, upper: Option<&[u8]>) -> IndexShard {
        IndexShard {
            shard_id,
            lower: lower.map(<[u8]>::to_vec),
            upper: upper.map(<[u8]>::to_vec),
            checkpoint_file: format!("{shard_id}.index").into(),
        }
    }

    fn manifest(epoch: u64, shards: Vec<IndexShard>) -> IndexManifest {
        IndexManifest::new(
            1,
            1,
            IndexGeneration::new(epoch),
            ManifestEpoch::new(epoch),
            shards,
        )
        .expect("manifest should be valid")
    }

    #[test]
    fn manifest_routes_half_open_ranges_and_prunes_queries() {
        let manifest = manifest(
            1,
            vec![shard(0, None, Some(b"m")), shard(1, Some(b"m"), None)],
        );
        assert_eq!(
            manifest.route_key(b"a").map(|shard| shard.shard_id),
            Some(0)
        );
        assert_eq!(
            manifest.route_key(b"m").map(|shard| shard.shard_id),
            Some(1)
        );

        let selected = manifest.select_shards(&PartitionSelector::KeyRange {
            lower: Some(b"b".to_vec()),
            upper: Some(b"m".to_vec()),
        });
        assert_eq!(
            selected
                .iter()
                .map(|shard| shard.shard_id)
                .collect::<Vec<_>>(),
            vec![0]
        );
    }

    #[test]
    fn manifest_rejects_range_gaps() {
        let result = IndexManifest::new(
            1,
            1,
            IndexGeneration::new(1),
            ManifestEpoch::new(1),
            vec![shard(0, None, Some(b"m")), shard(1, Some(b"n"), None)],
        );
        assert!(result.is_err());
    }

    #[test]
    fn reader_handle_fences_retired_generation_reclamation() {
        let catalog = ManifestCatalog::new(manifest(1, vec![shard(0, None, None)]))
            .expect("catalog should be valid");
        let old_reader = catalog.acquire();
        catalog
            .publish(manifest(
                2,
                vec![shard(0, None, Some(b"m")), shard(1, Some(b"m"), None)],
            ))
            .expect("new manifest should publish");

        assert!(catalog.take_reclaimable_files().is_empty());
        assert_eq!(catalog.stats().retired_generations, 1);
        drop(old_reader);
        assert_eq!(
            catalog.take_reclaimable_files(),
            vec![PathBuf::from("0.index")]
        );
        assert_eq!(catalog.stats().retired_generations, 0);
        assert_eq!(catalog.stats().reclaimed_files, 1);
    }

    #[test]
    fn persisted_manifest_roundtrips_and_rejects_unknown_version() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("manifest.json");
        let manifest = manifest(1, vec![shard(0, None, None)]);
        manifest.store(&path).expect("manifest should persist");
        assert_eq!(
            IndexManifest::load(&path).expect("manifest should load"),
            manifest
        );

        let mut unsupported = manifest;
        unsupported.format_version += 1;
        assert!(unsupported.validate().is_err());
    }
}
