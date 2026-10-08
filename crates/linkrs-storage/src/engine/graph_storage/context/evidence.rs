use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

use super::GraphStorageContext;
use linkrs_core::types::LabelId;

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

impl GraphStorageContext {
    // ── Layout version & vertex-id domain evidence ───────────────────────────

    /// Monotonic physical layout version. Bumped on compaction, restore, and
    /// remap so consumers can detect stale plans.
    pub(crate) fn layout_version(&self) -> u64 {
        self.persistent.layout_version.get()
    }

    /// Bump the monotonic physical layout version.
    pub(crate) fn bump_layout_version(&self) {
        self.persistent.layout_version.bump();
    }

    /// Observe an i64 vertex-id write for the space's self-proven domain.
    /// Negative ids are rejected by the write path; the domain evidence only
    /// trusts non-negative i64 ids and falls back to `None` otherwise.
    pub(crate) fn observe_vertex_id_i64(&self, label: LabelId, id: i64) {
        let evidence = self.vertex_id_domain_evidence(label);
        evidence.observe_i64(id);
    }

    /// Observe a non-numeric (string) vertex-id write. Any string id in a
    /// label invalidates the numeric domain evidence for that label.
    pub(crate) fn observe_vertex_id_string(&self, label: LabelId) {
        let evidence = self.vertex_id_domain_evidence(label);
        evidence.observe_string();
    }

    fn vertex_id_domain_evidence(&self, label: LabelId) -> Arc<VertexIdDomainEvidence> {
        let domains = &self.persistent.vertex_id_domains;
        if let Some(evidence) = domains.read().get(&label) {
            return Arc::clone(evidence);
        }
        let evidence = Arc::new(VertexIdDomainEvidence::new());
        domains
            .write()
            .entry(label)
            .or_insert_with(|| Arc::clone(&evidence));
        evidence
    }

    /// Union of the self-proven vertex-id domains across the space's labels.
    /// Returns `None` when any label with rows lacks evidence (mixed/string
    /// ids), since a guessed range could silently omit rows. Labels with no
    /// writes have no evidence entry and contribute nothing — they are
    /// skipped rather than blocking the whole space.
    pub(crate) fn vertex_id_domain(&self, space: &str) -> Option<std::ops::Range<i64>> {
        let tags = self.schema_manager().list_tags(space).ok()?;
        let domains = self.persistent.vertex_id_domains.read();
        let mut min = i64::MAX;
        let mut max = i64::MIN;
        for tag in &tags {
            let Some(evidence) = domains.get(&tag.tag_id) else {
                // No writes for this label: nothing to cover.
                continue;
            };
            let range = evidence.domain()?;
            min = min.min(range.start);
            max = max.max(range.end);
        }
        if min >= max {
            return None;
        }
        Some(min..max)
    }

    /// Rebuild the self-proven vertex-id domain evidence from the live vertex
    /// tables. Called after restore/checkpoint load where write-path
    /// accumulation did not run. Also bumps the layout version (a restore
    /// changes the physical layout).
    pub(crate) fn rebuild_vertex_id_domains(&self) {
        let tables = self
            .persistent
            .data_store
            .with_vertex_tables(|tables| tables.values().cloned().collect::<Vec<_>>());
        for table in tables {
            let label = table.label();
            let evidence = self.vertex_id_domain_evidence(label);
            for key in table.external_id_keys() {
                match key {
                    crate::vertex::IdKey::Int(id) => evidence.observe_i64(id),
                    crate::vertex::IdKey::Text(_) => evidence.observe_string(),
                }
            }
        }
        self.bump_layout_version();
    }
}
