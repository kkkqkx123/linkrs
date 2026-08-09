//! Slot coverage optimization rules.
//!
//! The columnar evaluator (`try_evaluate_columnar`) only hits when the
//! evaluated expression has a compound slot (`var.prop`) in the chunk
//! layout. Scan sources expose slots only for their `projected_properties`,
//! so a residual predicate directly above a scan falls back to per-row
//! evaluation when its condition references a column that is not projected
//! (gap G1 in `docs/plan/plan_columnar_fastpath_improvements.md`).
//!
//! Rules in this module widen the scan output layout by merging predicate
//! columns into `projected_properties`, without changing the output
//! contract: extra slots are private to the scan and are trimmed by the
//! upper `Project` node's `output_col_names`.

pub mod enrich_scan_slots_with_filter_props;

pub use enrich_scan_slots_with_filter_props::EnrichScanSlotsWithFilterPropsRule;