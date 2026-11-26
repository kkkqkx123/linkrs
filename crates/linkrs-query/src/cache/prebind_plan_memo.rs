//! Bounded memos for plans that can be identified before binding.
//!
//! Read queries and shape-normalized DML each own one instance of the same
//! component. Their keys differ, but lookup, bounded storage, checked serving
//! and counters are identical. Separate instances keep a hot namespace from
//! evicting plans in the other one.

use super::{PlanCacheContext, PlanCacheKey, QueryPlanCache};
use crate::executor::streaming::plan::PhysicalPlan;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Fingerprint-less plan identity shared by both pre-bind memos.
///
/// Partition fingerprints are only known after optimization, so they are
/// absent here. Serving validates the layout through the single checked exit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PrebindPlanKey {
    pub(crate) inner: PlanCacheKey,
}

impl PrebindPlanKey {
    pub(crate) fn from_context(query: &str, context: PlanCacheContext) -> Self {
        Self {
            inner: PlanCacheKey::fingerprint_less_from_context(query, context),
        }
    }
}

struct PrebindPlanEntry<K> {
    key: K,
    plan: Arc<PhysicalPlan>,
}

/// One namespace of pre-bound plans with linear move-to-front lookup.
///
/// The read and DML memos are separate instances so neither can evict the
/// other's entries. Hit, failure and bind-skip counters are likewise scoped
/// to the namespace that owns this instance.
pub(crate) struct PrebindPlanMemo<K: PartialEq> {
    entries: Mutex<Vec<PrebindPlanEntry<K>>>,
    capacity: usize,
    namespace: &'static str,
    hits: AtomicU64,
    failures: AtomicU64,
    bind_skips: AtomicU64,
}

impl<K: PartialEq> PrebindPlanMemo<K> {
    pub(crate) fn new(capacity: usize, namespace: &'static str) -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            capacity,
            namespace,
            hits: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            bind_skips: AtomicU64::new(0),
        }
    }

    /// Existence probe used before binding. Deliberately leaves entry order
    /// untouched: only the compile path serves a plan and counts a hit.
    pub(crate) fn contains(&self, key: &K) -> bool {
        self.entries.lock().iter().any(|entry| entry.key == *key)
    }

    /// Look up a candidate plan, moving the matching entry to the front.
    fn get(&self, key: &K) -> Option<Arc<PhysicalPlan>> {
        let mut guard = self.entries.lock();
        let position = guard.iter().position(|entry| entry.key == *key)?;
        if position > 0 {
            let entry = guard.remove(position);
            guard.insert(0, entry);
        }
        Some(guard[0].plan.clone())
    }

    /// Store a plan, replacing a same-key entry and evicting the least
    /// recently used entry past the capacity.
    pub(crate) fn store(&self, key: K, plan: Arc<PhysicalPlan>) {
        let mut guard = self.entries.lock();
        guard.retain(|entry| entry.key != key);
        guard.insert(0, PrebindPlanEntry { key, plan });
        guard.truncate(self.capacity);
    }

    /// Serve a memo candidate through the one checked exit. Layout failures
    /// fall through to replan and stay attributed to this namespace.
    pub(crate) fn serve_prebind(
        &self,
        key: &K,
        plan_cache: &QueryPlanCache,
        schema_version: Option<u64>,
    ) -> Option<Arc<PhysicalPlan>> {
        let plan = self.get(key)?;
        if plan_cache.serve_checked(&plan, schema_version).is_ok() {
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Some(plan);
        }
        self.record_failure();
        None
    }

    /// Attribute a checked-serve failure from a shared plan-cache path to
    /// this namespace.
    pub(crate) fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
        log::debug!(
            "Pre-bind plan ({}) failed checked serve, falling through to replan",
            self.namespace
        );
    }

    /// Record that this namespace's memo skipped binding during prepare.
    pub(crate) fn record_bind_skip(&self) {
        self.bind_skips.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    pub(crate) fn failures(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }

    pub(crate) fn bind_skips(&self) -> u64 {
        self.bind_skips.load(Ordering::Relaxed)
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.lock().len()
    }

    pub(crate) fn clear(&self) {
        self.entries.lock().clear();
    }
}
