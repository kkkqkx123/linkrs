use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use crate::core::types::{CommitLsn, IndexGeneration, SnapshotTimestamp};
use crate::core::{StorageError, StorageResult};
use crate::storage::cursor::PartitionSelector;

const MANIFEST_FORMAT_VERSION: u16 = 3;

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
}

/// Persisted tracking data for one native index generation build.
/// Stored alongside the index metadata so it survives crashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationBuildState {
    /// The new generation number being built.
    pub generation: IndexGeneration,
    /// MVCC timestamp used by the fixed snapshot scan.
    pub snapshot_timestamp: SnapshotTimestamp,
    /// WAL LSN at which the snapshot was taken. Catch-up replays entries > this.
    pub start_lsn: CommitLsn,
    /// WAL LSN at which the publish fence was established.
    /// All writes with LSN <= barrier_lsn are reflected in the new generation.
    pub barrier_lsn: Option<CommitLsn>,
    /// Current state in the Building → CatchingUp → Publishing → Active sequence.
    pub state: GenerationState,
}

impl GenerationBuildState {
    pub fn new(
        generation: IndexGeneration,
        snapshot_timestamp: SnapshotTimestamp,
        start_lsn: CommitLsn,
    ) -> Self {
        Self {
            generation,
            snapshot_timestamp,
            start_lsn,
            barrier_lsn: None,
            state: GenerationState::Building,
        }
    }

    pub fn transition_to_catching_up(&mut self) -> StorageResult<()> {
        self.require_state(GenerationState::Building)?;
        self.state = GenerationState::CatchingUp;
        Ok(())
    }

    pub fn transition_to_publishing(&mut self, barrier_lsn: CommitLsn) -> StorageResult<()> {
        self.require_state(GenerationState::CatchingUp)?;
        if barrier_lsn < self.start_lsn {
            return Err(StorageError::invalid_operation(
                "Generation barrier LSN precedes the snapshot LSN",
            ));
        }
        self.barrier_lsn = Some(barrier_lsn);
        self.state = GenerationState::Publishing;
        Ok(())
    }

    pub fn transition_to_active(&mut self) -> StorageResult<()> {
        self.require_state(GenerationState::Publishing)?;
        self.state = GenerationState::Active;
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.state == GenerationState::Active
    }

    fn require_state(&self, expected: GenerationState) -> StorageResult<()> {
        if self.state == expected {
            Ok(())
        } else {
            Err(StorageError::invalid_operation(format!(
                "Invalid generation transition from {:?}; expected {:?}",
                self.state, expected
            )))
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
    #[serde(default)]
    pub checksum: Option<u32>,
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

    fn validate(&self) -> StorageResult<()> {
        if self
            .lower
            .as_ref()
            .zip(self.upper.as_ref())
            .is_some_and(|(lower, upper)| lower >= upper)
        {
            return Err(StorageError::db_error(format!(
                "Index shard {} has an empty or inverted range",
                self.shard_id
            )));
        }
        if self.checkpoint_file.as_os_str().is_empty() {
            return Err(StorageError::db_error(format!(
                "Index shard {} has no checkpoint file",
                self.shard_id
            )));
        }
        Ok(())
    }

    /// Compute the integrity checksum of the referenced checkpoint.
    ///
    /// Only single-file checkpoints carry a stable digest: a persistent shard
    /// checkpoint is a directory whose contents (`forward_chunks/`,
    /// `reverse_chunks/`, `index.wal`) are rewritten in place while the
    /// generation is active, so a manifest-stored digest would go stale between
    /// stores. Directory corruption is instead detected at read time by the
    /// per-chunk CRC32 embedded in every chunk file (see `serialize.rs`).
    pub fn compute_checksum(&self) -> StorageResult<Option<u32>> {
        if self.checkpoint_file.as_os_str().is_empty() {
            return Ok(None);
        }
        if self.checkpoint_file.is_dir() {
            return Ok(None);
        }
        let data = match std::fs::read(&self.checkpoint_file) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(StorageError::db_error(format!(
                    "Read checkpoint for checksum: {}: {e}",
                    self.checkpoint_file.display()
                )));
            }
        };
        Ok(Some(crc32fast::hash(&data)))
    }

    pub fn verify_checksum(&self) -> StorageResult<()> {
        let Some(expected) = self.checksum else {
            return Ok(());
        };
        let Some(actual) = self.compute_checksum()? else {
            return Ok(());
        };
        if actual != expected {
            return Err(StorageError::db_error(format!(
                "Shard {} checksum mismatch: expected {expected:#010x}, got {actual:#010x}",
                self.shard_id
            )));
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
    pub shards: Vec<IndexShard>,
}

impl IndexManifest {
    pub fn new(
        space_id: u64,
        index_id: u64,
        generation: IndexGeneration,
        shards: Vec<IndexShard>,
    ) -> StorageResult<Self> {
        let manifest = Self {
            format_version: MANIFEST_FORMAT_VERSION,
            space_id,
            index_id,
            generation,
            shards,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> StorageResult<()> {
        if self.format_version != MANIFEST_FORMAT_VERSION {
            return Err(StorageError::db_error(format!(
                "Unsupported index manifest version {}",
                self.format_version
            )));
        }
        if self.shards.is_empty() {
            return Err(StorageError::db_error(
                "Index manifest must contain at least one shard",
            ));
        }
        if self
            .shards
            .first()
            .is_some_and(|shard| shard.lower.is_some())
        {
            return Err(StorageError::db_error(
                "The first index shard must have an unbounded lower range",
            ));
        }
        if self
            .shards
            .last()
            .is_some_and(|shard| shard.upper.is_some())
        {
            return Err(StorageError::db_error(
                "The last index shard must have an unbounded upper range",
            ));
        }
        for shard in &self.shards {
            shard.validate()?;
        }
        for pair in self.shards.windows(2) {
            if pair[0].shard_id == pair[1].shard_id {
                return Err(StorageError::db_error(format!(
                    "Duplicate index shard id {}",
                    pair[0].shard_id
                )));
            }
            if pair[0].upper != pair[1].lower {
                return Err(StorageError::db_error(format!(
                    "Index shards {} and {} are not contiguous",
                    pair[0].shard_id, pair[1].shard_id
                )));
            }
        }
        Ok(())
    }

    /// Route `key` to its owning shard via binary search over the sorted,
    /// contiguous shard ranges.
    pub fn route_key(&self, key: &[u8]) -> Option<&IndexShard> {
        let idx = self
            .shards
            .partition_point(|shard| shard.lower.as_deref().is_none_or(|lower| lower <= key));
        let shard = self.shards.get(idx.wrapping_sub(1))?;
        shard.contains(key).then_some(shard)
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
        self.scan_ranges_with_shard(selector, query_lower, query_upper)
            .into_iter()
            .map(|(_, lower, upper)| (lower, upper))
            .collect()
    }

    pub fn scan_ranges_with_shard(
        &self,
        selector: &PartitionSelector,
        query_lower: &[u8],
        query_upper: &[u8],
    ) -> Vec<(u32, Vec<u8>, Vec<u8>)> {
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
                (lower < upper).then_some((shard.shard_id, lower, upper))
            })
            .collect()
    }

    pub fn store(&self, path: &Path) -> StorageResult<()> {
        self.validate()?;
        let with_checksums = self.with_checksums()?;
        let bytes = postcard::to_allocvec(&with_checksums).map_err(|error| {
            StorageError::db_error(format!("Serialize index manifest: {error}"))
        })?;
        let mut versioned = Vec::new();
        crate::storage::persistence::write_versioned_payload(
            &mut versioned,
            crate::core::types::StorageVersion::CURRENT as u32,
            &bytes,
        );
        crate::storage::persistence::write_file_atomic(path, &versioned)
    }

    pub fn load(path: &Path) -> StorageResult<Self> {
        let mut file = std::fs::File::open(path)?;
        let (_version, payload) = crate::storage::persistence::read_versioned_payload(
            &mut file,
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("manifest.bin"),
        )?;
        let manifest: Self = postcard::from_bytes(&payload)
            .map_err(|error| StorageError::db_error(format!("Read index manifest: {error}")))?;
        manifest.validate()?;
        for shard in &manifest.shards {
            shard.verify_checksum()?;
        }
        Ok(manifest)
    }

    /// Returns a clone with checksums computed from checkpoint files.
    pub fn with_checksums(&self) -> StorageResult<Self> {
        let mut clone = self.clone();
        for shard in &mut clone.shards {
            shard.checksum = shard.compute_checksum()?;
        }
        Ok(clone)
    }
}

/// A cursor-owned pin that prevents reclamation of a generation's physical files.
///
/// # Arc-count invariant
///
/// The strong count of the wrapped `Arc<IndexManifest>` is the authoritative
/// reader reference count: exactly one reference is held by
/// [`ManifestCatalog`] (the `active` slot or a `retired` entry) and every
/// additional reference is owned by a live handle produced via
/// [`ManifestCatalog::acquire`] or [`ManifestCatalog::acquire_generation`].
/// A retired generation is therefore reclaimable exactly when its count is 1
/// (only the catalog's own entry). Code outside this module must never clone
/// a manifest `Arc` directly — always go through the catalog so reclamation
/// stays sound.
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
    retired_at: Instant,
}

/// Publishes immutable manifests and fences reclamation with reader handles.
///
/// Reclamation contract: a retired manifest may be removed from the catalog
/// only when no reader handle references it (see [`ManifestHandle`]). The
/// physical checkpoint files of a generation additionally require that the
/// generation is no longer installed in the runtime — the caller coordinates
/// both conditions (see `IndexDataManagerImpl::reclaim_retired_generations`).
/// The catalog never drops a reader-pinned entry, so a live handle can always
/// be re-acquired for any generation still present in the retired list.
#[derive(Debug)]
pub struct ManifestCatalog {
    active: RwLock<Arc<IndexManifest>>,
    retired: Mutex<Vec<RetiredManifest>>,
    published: AtomicU64,
    reclaimed_files: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestCatalogStats {
    pub active_generation: IndexGeneration,
    pub active_readers: u64,
    pub retired_generations: u64,
    pub published_manifests: u64,
    pub reclaimed_files: u64,
    pub oldest_retired_age_secs: u64,
}

impl ManifestCatalog {
    pub fn new(manifest: IndexManifest) -> StorageResult<Self> {
        manifest.validate()?;
        Ok(Self {
            active: RwLock::new(Arc::new(manifest)),
            retired: Mutex::new(Vec::new()),
            published: AtomicU64::new(0),
            reclaimed_files: AtomicU64::new(0),
        })
    }

    /// Acquire a pin on the active manifest.
    pub fn acquire(&self) -> ManifestHandle {
        let guard = self.active.read();
        #[cfg(debug_assertions)]
        {
            let count = Arc::strong_count(&guard);
            log::trace!("ManifestCatalog acquire: readers={}", count + 1);
        }
        ManifestHandle(Arc::clone(&guard))
    }

    /// Acquire a pin on the manifest of a specific generation, whether it is
    /// currently active or already retired. This lets cursors fence the whole
    /// generation chain (parents included) from reclamation.
    pub fn acquire_generation(&self, generation: IndexGeneration) -> Option<ManifestHandle> {
        {
            let active = self.active.read();
            if active.generation == generation {
                return Some(ManifestHandle(Arc::clone(&active)));
            }
        }
        let retired = self.retired.lock();
        retired
            .iter()
            .find(|entry| entry.manifest.generation == generation)
            .map(|entry| ManifestHandle(Arc::clone(&entry.manifest)))
    }

    pub fn publish(&self, manifest: IndexManifest) -> StorageResult<ManifestHandle> {
        manifest.validate()?;
        let mut active = self.active.write();
        if manifest.index_id != active.index_id {
            return Err(StorageError::invalid_operation(
                "Cannot publish a manifest for another index",
            ));
        }
        if manifest.generation <= active.generation {
            return Err(StorageError::invalid_operation(
                "Index generation must increase",
            ));
        }

        let next = Arc::new(manifest);
        let previous = std::mem::replace(&mut *active, Arc::clone(&next));
        #[cfg(debug_assertions)]
        {
            log::trace!(
                "ManifestCatalog publish: gen={}, active_readers={}",
                next.generation,
                Arc::strong_count(&next) - 1,
            );
        }

        // Retire the previous manifest without dropping it: reader-pinned
        // entries must stay tracked so their files remain reclaimable later
        // and so `acquire_generation` can re-pin them. Reclamation is driven
        // by the caller (see `IndexDataManagerImpl::reclaim_retired_generations`),
        // not by a bounded sweep here.
        self.retired.lock().push(RetiredManifest {
            manifest: previous,
            retired_at: Instant::now(),
        });
        self.published.fetch_add(1, Ordering::Relaxed);
        Ok(ManifestHandle(next))
    }

    /// Returns files from retired generations that no longer have any reader
    /// handle. The caller is responsible for deleting the physical files.
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

    /// Remove and return every retired manifest whose last reader handle is
    /// gone (Arc count of 1). Entries still referenced by a live handle are
    /// kept so their files remain trackable.
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
        let total = manifests.iter().map(|m| m.shards.len() as u64).sum();
        self.reclaimed_files.fetch_add(total, Ordering::Relaxed);
        manifests
    }

    /// Peek (without removing) the retired manifests that have no reader
    /// handle and satisfy `matches`. The caller decides whether the physical
    /// files can actually be deleted (e.g. the generation is no longer
    /// installed in the runtime) and then removes them with
    /// [`remove_retired`](Self::remove_retired).
    pub fn retired_reclaimable<F>(&self, matches: F) -> Vec<IndexManifest>
    where
        F: Fn(&IndexManifest) -> bool,
    {
        let retired = self.retired.lock();
        retired
            .iter()
            .filter(|entry| Arc::strong_count(&entry.manifest) == 1 && matches(&entry.manifest))
            .map(|entry| (*entry.manifest).clone())
            .collect()
    }

    /// Forget a retired manifest after its physical files have been reclaimed.
    pub fn remove_retired(&self, generation: IndexGeneration) -> bool {
        let mut retired = self.retired.lock();
        let before = retired.len();
        retired.retain(|entry| entry.manifest.generation != generation);
        retired.len() != before
    }

    pub fn stats(&self) -> ManifestCatalogStats {
        let active = self.active.read();
        let retired = self.retired.lock();
        let now = Instant::now();
        let oldest_age = retired
            .iter()
            .map(|e| now.duration_since(e.retired_at).as_secs())
            .max()
            .unwrap_or(0);
        ManifestCatalogStats {
            active_generation: active.generation,
            active_readers: Arc::strong_count(&active).saturating_sub(1) as u64,
            retired_generations: retired.len() as u64,
            published_manifests: self.published.load(Ordering::Relaxed),
            reclaimed_files: self.reclaimed_files.load(Ordering::Relaxed),
            oldest_retired_age_secs: oldest_age,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{IndexManifest, IndexShard, ManifestCatalog};
    use crate::core::types::IndexGeneration;
    use crate::storage::cursor::PartitionSelector;

    fn shard(shard_id: u32, lower: Option<&[u8]>, upper: Option<&[u8]>) -> IndexShard {
        IndexShard {
            shard_id,
            lower: lower.map(<[u8]>::to_vec),
            upper: upper.map(<[u8]>::to_vec),
            checkpoint_file: format!("{shard_id}.index").into(),
            checksum: None,
        }
    }

    fn manifest(generation: u64, shards: Vec<IndexShard>) -> IndexManifest {
        IndexManifest::new(1, 1, IndexGeneration::new(generation), shards)
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
            vec![shard(0, None, Some(b"m")), shard(1, Some(b"n"), None)],
        );
        assert!(result.is_err());
    }

    #[test]
    fn reader_pin_keeps_retired_manifest_tracked_until_last_handle_drops() {
        let catalog = ManifestCatalog::new(manifest(1, vec![shard(0, None, None)]))
            .expect("catalog should be valid");
        let old_reader = catalog.acquire();
        catalog
            .publish(manifest(
                2,
                vec![shard(0, None, Some(b"m")), shard(1, Some(b"m"), None)],
            ))
            .expect("new manifest should publish");

        // The pinned generation 1 must stay in the retired list and be
        // re-acquirable even though it has no dedicated reader handle.
        assert!(catalog.take_reclaimable_files().is_empty());
        assert_eq!(catalog.stats().retired_generations, 1);
        let pin = catalog
            .acquire_generation(IndexGeneration::new(1))
            .expect("retired generation should be re-acquirable");
        drop(pin);
        assert!(catalog.take_reclaimable_files().is_empty());

        drop(old_reader);
        assert_eq!(
            catalog.take_reclaimable_files(),
            vec![PathBuf::from("0.index")]
        );
        assert_eq!(catalog.stats().retired_generations, 0);
        assert_eq!(catalog.stats().reclaimed_files, 1);
    }

    #[test]
    fn retired_reclaimable_filters_by_caller_predicate() {
        let catalog = ManifestCatalog::new(manifest(1, vec![shard(0, None, None)]))
            .expect("catalog should be valid");
        let old_reader = catalog.acquire();
        catalog
            .publish(manifest(
                2,
                vec![shard(0, None, Some(b"m")), shard(1, Some(b"m"), None)],
            ))
            .expect("new manifest should publish");
        catalog
            .publish(manifest(3, vec![shard(0, None, None)]))
            .expect("new manifest should publish");

        // Generation 1 is still reader-pinned; only generation 2 matches.
        let candidates = catalog.retired_reclaimable(|m| m.generation.get() == 2);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].generation.get(), 2);

        // Peeking must not remove anything.
        assert_eq!(catalog.stats().retired_generations, 2);

        assert!(catalog.remove_retired(IndexGeneration::new(2)));
        assert!(!catalog.remove_retired(IndexGeneration::new(99)));
        assert_eq!(catalog.stats().retired_generations, 1);

        drop(old_reader);
        catalog.take_reclaimable_manifests();
        assert_eq!(catalog.stats().retired_generations, 0);
    }

    #[test]
    fn published_handle_still_reads_old_manifest_until_released() {
        let catalog = ManifestCatalog::new(manifest(1, vec![shard(0, None, None)]))
            .expect("catalog should be valid");
        let first = catalog.acquire();
        let second = catalog
            .publish(manifest(2, vec![shard(0, None, None)]))
            .expect("new manifest should publish");

        // Publishing returns a handle to the new manifest; the old handle
        // still references generation 1 and fences it from reclamation.
        assert_eq!(second.manifest().generation.get(), 2);
        assert_eq!(first.manifest().generation.get(), 1);
        assert!(catalog.take_reclaimable_files().is_empty());

        drop(first);
        assert_eq!(
            catalog.take_reclaimable_files(),
            vec![PathBuf::from("0.index")]
        );
    }

    #[test]
    fn persisted_manifest_roundtrips_and_rejects_unknown_version() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let path = directory.path().join("manifest.bin");
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

    #[test]
    fn manifest_checksum_verifies_on_load() {
        let directory = tempfile::tempdir().expect("temporary directory should be created");
        let checkpoint_path = directory.path().join("checkpoint.bin");
        std::fs::write(&checkpoint_path, b"test checkpoint data").expect("write checkpoint");

        let shard_obj = IndexShard {
            shard_id: 0,
            lower: None,
            upper: None,
            checkpoint_file: checkpoint_path.clone(),
            checksum: None,
        };
        let manifest = IndexManifest::new(1, 1, IndexGeneration::new(1), vec![shard_obj])
            .expect("manifest should be valid");

        let manifest_path = directory.path().join("manifest.bin");
        manifest
            .store(&manifest_path)
            .expect("store should succeed");

        let loaded = IndexManifest::load(&manifest_path).expect("load should succeed");
        assert!(loaded.shards[0].checksum.is_some());

        std::fs::write(&checkpoint_path, b"corrupted data").expect("corrupt checkpoint");
        assert!(IndexManifest::load(&manifest_path).is_err());
    }
}
