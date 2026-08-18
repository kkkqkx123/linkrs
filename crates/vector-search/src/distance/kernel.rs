//! SIMD kernel selection.
//!
//! Mirror of tantivy's per-instruction-set dispatch
//! (`crates/tantivy/bitpacker/src/filter_vec/mod.rs`): an enum of kernel
//! variants (cfg-gated per architecture), a preferred-order list per
//! architecture, a single cached best-available selection, and a tiny
//! dispatch. Call sites go through [`distance`](super::distance) and never
//! change when a new architecture or instruction set is added (adding one is
//! one enum variant + one `is_available` arm + one `distance` arm + one
//! `IMPLS` entry).
//!
//! Note: on builds compiled with `-C target-cpu=x86-64-v3` (the workspace
//! default, `.cargo/config.toml`) the whole binary requires AVX2 hardware and
//! `Avx2` is always selected; the runtime checks only matter for baseline
//! (`x86_64`) builds targeting older CPUs.

use std::fmt;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::OnceLock;

use crate::types::DistanceMetric;

#[cfg(target_arch = "x86_64")]
use super::avx2;
use super::naive;

/// A distance kernel implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kernel {
    /// Scalar baseline; always available, always correct.
    Naive,
    /// AVX2 + FMA kernels (x86-64 only).
    #[cfg(target_arch = "x86_64")]
    Avx2,
}

impl Kernel {
    /// Whether this kernel can run on the current CPU.
    pub fn is_available(self) -> bool {
        match self {
            Kernel::Naive => true,
            #[cfg(target_arch = "x86_64")]
            Kernel::Avx2 => {
                std::arch::is_x86_feature_detected!("avx2")
                    && std::arch::is_x86_feature_detected!("fma")
            }
        }
    }

    /// Internal distance between two vectors (smaller = nearer).
    #[inline]
    pub fn distance(self, metric: DistanceMetric, a: &[f32], b: &[f32]) -> f32 {
        match self {
            Kernel::Naive => naive::distance(metric, a, b),
            #[cfg(target_arch = "x86_64")]
            Kernel::Avx2 => unsafe { avx2::distance(metric, a, b) },
        }
    }
}

impl fmt::Display for Kernel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Kernel::Naive => write!(f, "naive"),
            #[cfg(target_arch = "x86_64")]
            Kernel::Avx2 => write!(f, "avx2"),
        }
    }
}

/// Preferred kernel order per architecture (mirrors tantivy's `IMPLS`).
#[cfg(target_arch = "x86_64")]
const IMPLS: [Kernel; 2] = [Kernel::Avx2, Kernel::Naive];
#[cfg(not(target_arch = "x86_64"))]
const IMPLS: [Kernel; 1] = [Kernel::Naive];

fn best_available() -> Kernel {
    IMPLS
        .into_iter()
        .find(|k| k.is_available())
        .unwrap_or(Kernel::Naive)
}

static SELECTED: OnceLock<Kernel> = OnceLock::new();

/// Test-only override; wins over the cached detection so differential tests
/// can pin a specific kernel deterministically regardless of test order.
#[cfg(test)]
static OVERRIDE: Mutex<Option<Kernel>> = Mutex::new(None);

/// The kernel active for this process (first call detects and caches).
pub fn selected() -> Kernel {
    #[cfg(test)]
    {
        let guard = OVERRIDE.lock().expect("kernel override lock poisoned");
        if let Some(k) = *guard {
            return k;
        }
    }
    *SELECTED.get_or_init(best_available)
}

/// Pin the active kernel for differential debugging; tests only.
#[cfg(test)]
pub fn force_for_test(kernel: Kernel) {
    *OVERRIDE.lock().expect("kernel override lock poisoned") = Some(kernel);
}
