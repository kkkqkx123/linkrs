//! Shim over the single bench-group implementation in linkrs-core.
//!
//! The canonical windows and fast-run flag live in
//! `linkrs-core/benches/bench_group.rs`; this file only includes it so the
//! three crates cannot drift.
include!("../../linkrs-core/benches/bench_group.rs");
