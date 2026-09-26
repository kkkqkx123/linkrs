//! Flush trigger policy: when an incremental flush should anchor a full
//! baseline, when page merging is due, and when flushing is not worthwhile.
//!
//! Pure decision engine: signals in, plan out, no I/O, no locks. It changes
//! timing only — the commit-point contract (files first, manifest last) is
//! identical whatever the plan says. Callers log the reason code with every
//! full rewrite so operations can explain each anchor; the counters live
//! with the caller because this module holds no state.

use crate::vertex::id_indexer::IdManager;

/// Milliseconds between full baselines beyond which the next flush anchors
/// a new baseline even when the delta is small. Bounds recovery replay on
/// quiet tables whose delta would otherwise never trip the count threshold.
pub const MAX_BASELINE_AGE_MS: u64 = 300_000;

/// Minimum baseline age honored by the skip branch: a table flushed more
/// recently than this with no delta and low fragmentation is not worth
/// touching. Keeps tiny deployments from spinning on age alone.
pub const MIN_BASELINE_AGE_MS: u64 = 30_000;

/// Dirty-page ratio at or above which the plan asks for a page merge. The
/// ratio is dirty over total column pages, a cheap proxy for shadow-page
/// fragmentation that needs no directory walk at decision time.
pub const PAGE_MERGE_RATIO: f64 = 0.5;

/// Recommended flush action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushKind {
    /// Nothing worth persisting: no delta, young baseline, clean pages.
    Skip,
    /// Persist the since-baseline delta onto the existing baseline.
    Incremental,
    /// Rewrite the full baseline and clear the delta.
    Full,
}

/// Machine-readable reason for a [`FlushPlan`], logged with every flush so
/// each full rewrite is explainable after the fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlushReason {
    /// Skip branch: quiescent table.
    Idle,
    /// Incremental branch: unanchored delta pending.
    DeltaPending,
    /// Full branch: compaction moved rows after the last baseline, so
    /// delta addressing by stale ids is invalid.
    CompactionMovedRows,
    /// Full branch: delta replay cost passed the scale-aware threshold.
    DeltaThreshold,
    /// Full branch: baseline older than [`MAX_BASELINE_AGE_MS`].
    BaselineAge,
    /// Incremental branch with merge: fragmentation passed
    /// [`PAGE_MERGE_RATIO`] but the delta does not justify a rewrite.
    PageFragmentation,
}

impl FlushReason {
    /// Stable snake-case code for logs and metrics labels.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::DeltaPending => "delta_pending",
            Self::CompactionMovedRows => "compaction_moved_rows",
            Self::DeltaThreshold => "delta_threshold",
            Self::BaselineAge => "baseline_age",
            Self::PageFragmentation => "page_fragmentation",
        }
    }

    /// All variants in stable order for reason-distribution counting.
    pub fn all() -> [Self; 6] {
        [
            Self::Idle,
            Self::DeltaPending,
            Self::CompactionMovedRows,
            Self::DeltaThreshold,
            Self::BaselineAge,
            Self::PageFragmentation,
        ]
    }
}

/// Observed table state feeding [`decide`].
#[derive(Debug, Clone, Copy)]
pub struct FlushSignals {
    /// Committed index mutations since the last baseline.
    pub delta_entries: usize,
    /// Live rows scaling the delta threshold.
    pub live_rows: usize,
    /// Whether compaction moved rows after the last baseline.
    pub baseline_invalidated: bool,
    /// Milliseconds since the last full baseline.
    pub millis_since_baseline: u64,
    /// Dirty over total column pages (`decide` clamps to 0..=1).
    pub dirty_page_ratio: f64,
}

/// One trigger decision: what to flush, why, and whether to merge pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlushPlan {
    pub kind: FlushKind,
    pub reason: FlushReason,
    /// True when page consolidation is due regardless of kind. Decoupled
    /// from the rewrite so small tables are not forced into full baselines
    /// by fragmentation alone.
    pub merge_pages: bool,
}

/// Decide the flush action for `signals`.
///
/// Priority is correctness first: an invalidated baseline forces a full
/// rewrite before any cost comparison, because deltas addressed by stale
/// ids are unusable. Cost branches follow in replay-cost order, and the
/// skip branch only fires when every signal agrees the table is quiet.
/// Parameters carry hard upper bounds but no lower adaptation: under
/// extreme writes the policy degrades to threshold behavior instead of
/// chasing the load.
pub fn decide(signals: FlushSignals) -> FlushPlan {
    // Non-finite ratios come from empty tables (0/0); treat them as clean
    // rather than letting NaN comparisons silently pick a branch.
    let fragmentation = if signals.dirty_page_ratio.is_finite() {
        signals.dirty_page_ratio.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let merge_pages = fragmentation >= PAGE_MERGE_RATIO;
    if signals.baseline_invalidated {
        return FlushPlan {
            kind: FlushKind::Full,
            reason: FlushReason::CompactionMovedRows,
            merge_pages,
        };
    }
    if signals.delta_entries >= IdManager::anchor_threshold_for_live(signals.live_rows) {
        return FlushPlan {
            kind: FlushKind::Full,
            reason: FlushReason::DeltaThreshold,
            merge_pages,
        };
    }
    if signals.millis_since_baseline >= MAX_BASELINE_AGE_MS {
        return FlushPlan {
            kind: FlushKind::Full,
            reason: FlushReason::BaselineAge,
            merge_pages,
        };
    }
    if signals.delta_entries == 0
        && signals.millis_since_baseline < MIN_BASELINE_AGE_MS
        && !merge_pages
    {
        return FlushPlan {
            kind: FlushKind::Skip,
            reason: FlushReason::Idle,
            merge_pages: false,
        };
    }
    FlushPlan {
        kind: FlushKind::Incremental,
        reason: if merge_pages {
            FlushReason::PageFragmentation
        } else {
            FlushReason::DeltaPending
        },
        merge_pages,
    }
}

/// Whole-checkpoint flush-kind policy: the strategy-selection layer turns
/// per-table verdicts plus reason counts into one global kind.
///
/// Exactly two outcomes: any table overdue for a baseline escalates the
/// whole checkpoint to full, otherwise the checkpoint stays incremental.
/// Single-table upgrades stay forbidden: the flush loop must publish the
/// returned kind for every table and refuse a table that flips to full
/// underneath a globally incremental checkpoint (see
/// [`check_single_table_kind`]).
pub fn select_whole_db_kind(plans: &[FlushPlan]) -> (FlushKind, [(FlushReason, usize); 6]) {
    let mut counts = [
        (FlushReason::Idle, 0usize),
        (FlushReason::DeltaPending, 0usize),
        (FlushReason::CompactionMovedRows, 0usize),
        (FlushReason::DeltaThreshold, 0usize),
        (FlushReason::BaselineAge, 0usize),
        (FlushReason::PageFragmentation, 0usize),
    ];
    for plan in plans {
        if let Some(slot) = counts.iter_mut().find(|(reason, _)| *reason == plan.reason) {
            slot.1 += 1;
        }
    }
    let kind = if plans.iter().any(|plan| plan.kind == FlushKind::Full) {
        FlushKind::Full
    } else {
        FlushKind::Incremental
    };
    (kind, counts)
}

/// Single-table upgrade guard: a table verdict of full inside a globally
/// incremental checkpoint must error, never silently upgrade. Upgrading
/// one table while the checkpoint keeps its global incremental identity
/// splits the recovery contract: the full table directory is misused when
/// recovery overlays deltas onto it.
pub fn check_single_table_kind(
    table: &str,
    table_kind: FlushKind,
    global_kind: FlushKind,
    reason: FlushReason,
) -> graphdb_core::StorageResult<()> {
    if table_kind == FlushKind::Full && global_kind == FlushKind::Incremental {
        return Err(graphdb_core::StorageError::invalid_operation(format!(
            "vertex table '{table}' verdict is full (reason={}) inside a globally \
             incremental checkpoint: single-table upgrades are forbidden; escalate \
             the whole checkpoint through flush policy selection instead",
            reason.as_str(),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> FlushSignals {
        FlushSignals {
            delta_entries: 0,
            live_rows: 100,
            baseline_invalidated: false,
            millis_since_baseline: 1_000,
            dirty_page_ratio: 0.0,
        }
    }

    #[test]
    fn test_quiet_table_skips() {
        let plan = decide(quiet());
        assert_eq!(plan.kind, FlushKind::Skip);
        assert_eq!(plan.reason, FlushReason::Idle);
        assert!(!plan.merge_pages);
    }

    #[test]
    fn test_invalidated_baseline_forces_full_first() {
        let plan = decide(FlushSignals {
            baseline_invalidated: true,
            ..quiet()
        });
        assert_eq!(plan.kind, FlushKind::Full);
        assert_eq!(plan.reason, FlushReason::CompactionMovedRows);
    }

    #[test]
    fn test_delta_threshold_forces_full() {
        let live = 100usize;
        let plan = decide(FlushSignals {
            delta_entries: IdManager::anchor_threshold_for_live(live),
            live_rows: live,
            ..quiet()
        });
        assert_eq!(plan.kind, FlushKind::Full);
        assert_eq!(plan.reason, FlushReason::DeltaThreshold);
    }

    #[test]
    fn test_old_baseline_forces_full() {
        let plan = decide(FlushSignals {
            millis_since_baseline: MAX_BASELINE_AGE_MS,
            ..quiet()
        });
        assert_eq!(plan.kind, FlushKind::Full);
        assert_eq!(plan.reason, FlushReason::BaselineAge);
    }

    #[test]
    fn test_fragmentation_merges_without_rewrite() {
        let plan = decide(FlushSignals {
            delta_entries: 5,
            dirty_page_ratio: 0.8,
            ..quiet()
        });
        assert_eq!(plan.kind, FlushKind::Incremental);
        assert_eq!(plan.reason, FlushReason::PageFragmentation);
        assert!(plan.merge_pages);
    }

    #[test]
    fn test_ratio_clamped_against_garbage() {
        let plan = decide(FlushSignals {
            dirty_page_ratio: f64::NAN,
            ..quiet()
        });
        assert_eq!(plan.kind, FlushKind::Skip);
        let plan = decide(FlushSignals {
            dirty_page_ratio: 2.0,
            ..quiet()
        });
        assert!(plan.merge_pages);
    }

    #[test]
    fn test_reason_codes_stable() {
        assert_eq!(FlushReason::DeltaThreshold.as_str(), "delta_threshold");
        assert_eq!(FlushReason::BaselineAge.as_str(), "baseline_age");
    }

    #[test]
    fn test_whole_db_kind_escalates_on_any_full() {
        let incremental = FlushPlan {
            kind: FlushKind::Incremental,
            reason: FlushReason::DeltaPending,
            merge_pages: false,
        };
        let (kind, counts) = select_whole_db_kind(&[incremental, incremental]);
        assert_eq!(kind, FlushKind::Incremental);
        assert_eq!(
            counts
                .iter()
                .find(|(reason, _)| *reason == FlushReason::DeltaPending)
                .map(|(_, count)| *count),
            Some(2)
        );
        let full = FlushPlan {
            kind: FlushKind::Full,
            reason: FlushReason::BaselineAge,
            merge_pages: false,
        };
        let (kind, _) = select_whole_db_kind(&[incremental, full]);
        assert_eq!(kind, FlushKind::Full);
    }

    #[test]
    fn test_single_table_upgrade_is_forbidden() {
        assert!(check_single_table_kind(
            "person",
            FlushKind::Full,
            FlushKind::Incremental,
            FlushReason::BaselineAge,
        )
        .is_err());
        assert!(check_single_table_kind(
            "person",
            FlushKind::Full,
            FlushKind::Full,
            FlushReason::BaselineAge,
        )
        .is_ok());
        assert!(check_single_table_kind(
            "person",
            FlushKind::Incremental,
            FlushKind::Incremental,
            FlushReason::DeltaPending,
        )
        .is_ok());
    }
}
