//! Central registry of storage-layer tunable thresholds.
//!
//! Every tunable threshold in the storage engine is listed in the domain
//! slices under [`entries`] with its unit, default value, tuning
//! surface(s), covering benchmarks, and the modules affected by a retune.
//! The gate tests in [`tests`] pin each constant (renaming one breaks the
//! build), assert that every documented default matches the live value,
//! and verify that linked benchmark functions still exist and stay
//! registered.
//!
//! Tuning surfaces:
//! - `config_file`: a `[storage]` key in `config.toml`, deserialized into
//!   `linkrs-config::StorageConfig` and mapped at server startup.
//! - `engine_struct`: a field on an engine-side config struct
//!   (constructed directly in code, or seeded from `StorageConfig`).
//! - `hardcoded`: a constant in this crate.
//!
//! Registry policy: a threshold either links one or more benchmark
//! functions that observe it, or carries a `note` explaining why no bench
//! can. Documented defaults are asserted equal to live values, so retuning
//! a constant or a config default without updating the table fails tests.
//! Coverage baseline is the storage threshold analysis inventory plus the
//! process-level memory watermark constants.

mod bench_ref;
mod entries;
mod model;
mod tests;
