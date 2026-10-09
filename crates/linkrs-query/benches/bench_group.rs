//! Criterion group setup shared by the benchmark binaries.
//!
//! Included per target with `#[path = "bench_group.rs"] mod bench_group;`
//! because a benchmark crate root cannot be shared otherwise: cargo compiles
//! each `[[bench]]` as its own crate, so a helper has to be pulled in by
//! path.

use criterion::Criterion;
use std::time::Duration;

/// Open a criterion group with the shared warm-up and measurement windows.
///
/// `sample_size` is the only per-suite knob: a suite that wants more
/// statistical confidence raises it, and one that keeps each sample expensive
/// lowers it. The windows stay identical so numbers from different suites
/// remain comparable.
pub fn create_benchmark_group<'a>(
    c: &'a mut Criterion,
    name: &str,
    sample_size: usize,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut group = c.benchmark_group(name);
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(sample_size);
    group.warm_up_time(Duration::from_secs(1));
    group
}
