use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

/// Monotonic counter for the physical vertex/edge layout.
///
/// Bumped whenever segment allocation, merge, compaction, eviction, or
/// restore changes the on-disk/in-memory layout of vertex or edge tables.
/// Consumers (e.g. the query plan cache) compare this version to detect
/// stale plans that assumed an older layout.
pub(crate) struct LayoutVersion {
    value: Arc<AtomicU64>,
}

impl LayoutVersion {
    pub(crate) fn new() -> Self {
        Self {
            value: Arc::new(AtomicU64::new(1)),
        }
    }

    pub(crate) fn get(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    pub(crate) fn bump(&self) {
        self.value.fetch_add(1, Ordering::Relaxed);
    }
}

impl Clone for LayoutVersion {
    fn clone(&self) -> Self {
        Self {
            value: Arc::clone(&self.value),
        }
    }
}

impl std::fmt::Debug for LayoutVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutVersion")
            .field("value", &self.get())
            .finish()
    }
}

/// Per-label vertex-id domain evidence.
///
/// The partition planner requires a vertex-id range that provably covers the
/// scanned domain; guessing a range can silently omit rows. This evidence is
/// accumulated on every write (and rebuilt after restore) so the storage can
/// self-prove a covering `[min, max]` range when *all* vertex ids of the
/// label are numeric and non-negative.
#[derive(Debug)]
pub(crate) struct VertexIdDomainEvidence {
    min_id: AtomicI64,
    max_id: AtomicI64,
    saw_string_id: AtomicBool,
}

impl VertexIdDomainEvidence {
    pub(crate) fn new() -> Self {
        Self {
            min_id: AtomicI64::new(i64::MAX),
            max_id: AtomicI64::new(i64::MIN),
            saw_string_id: AtomicBool::new(false),
        }
    }

    pub(crate) fn observe_i64(&self, id: i64) {
        self.min_id.fetch_min(id, Ordering::Relaxed);
        self.max_id.fetch_max(id, Ordering::Relaxed);
    }

    pub(crate) fn observe_string(&self) {
        self.saw_string_id.store(true, Ordering::Relaxed);
    }

    pub(crate) fn domain(&self) -> Option<std::ops::Range<i64>> {
        if self.saw_string_id.load(Ordering::Relaxed) {
            return None;
        }
        let min = self.min_id.load(Ordering::Relaxed);
        let max = self.max_id.load(Ordering::Relaxed);
        if min > max {
            return None;
        }
        Some(min..max.saturating_add(1))
    }
}
