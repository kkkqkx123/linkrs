//! AVX2 distance kernels (x86-64 only).
//!
//! Selection happens once per process in [`super::kernel`]: on
//! `.cargo/config.toml` `x86-64-v3` builds the AVX2+FMA path is always hit
//! (the whole binary requires AVX2 hardware anyway); on baseline `x86_64`
//! builds the runtime `is_x86_feature_detected!` checks in `Kernel::Avx2`
//! guard against older CPUs.
//!
//! Each kernel processes 8 f32 per YMM register with a scalar tail; cosine
//! keeps three accumulators (dot, norm a, norm b) in one loop. Loads are
//! unaligned (`_mm256_loadu_ps`), so any alignment works.

use crate::types::DistanceMetric;

use std::arch::x86_64::*;

/// Horizontally reduce a YMM lane sum to a scalar.
///
/// # Safety
///
/// Requires AVX at the call site (`_mm256_extractf128_ps`/`_mm256_castps256_ps128`
/// are AVX intrinsics); all callers are `#[target_feature(enable = "avx2,fma")]`
/// functions.
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn horizontal_sum(v: __m256) -> f32 {
    let hi = _mm256_extractf128_ps(v, 1);
    let lo = _mm256_castps256_ps128(v);
    let sum = _mm_add_ps(hi, lo);
    let sum = _mm_hadd_ps(sum, sum);
    let sum = _mm_hadd_ps(sum, sum);
    _mm_cvtss_f32(sum)
}

/// Squared Euclidean distance.
///
/// # Safety
///
/// The caller must ensure `a` and `b` have the same length, and that the
/// current CPU supports AVX2 + FMA (checked by `is_x86_feature_detected!`
/// before dispatch).
#[target_feature(enable = "avx2,fma")]
pub unsafe fn distance_l2(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = _mm256_setzero_ps();
    let mut i = 0;
    while i + 8 <= a.len() {
        let av = _mm256_loadu_ps(a.as_ptr().add(i));
        let bv = _mm256_loadu_ps(b.as_ptr().add(i));
        let d = _mm256_sub_ps(av, bv);
        acc = _mm256_fmadd_ps(d, d, acc);
        i += 8;
    }
    let mut sum = horizontal_sum(acc);
    for j in i..a.len() {
        let d = a[j] - b[j];
        sum += d * d;
    }
    sum
}

/// Dot product.
///
/// # Safety
///
/// The caller must ensure `a` and `b` have the same length, and that the
/// current CPU supports AVX2 + FMA (checked by `is_x86_feature_detected!`
/// before dispatch).
#[target_feature(enable = "avx2,fma")]
pub unsafe fn inner_product(a: &[f32], b: &[f32]) -> f32 {
    let mut acc = _mm256_setzero_ps();
    let mut i = 0;
    while i + 8 <= a.len() {
        let av = _mm256_loadu_ps(a.as_ptr().add(i));
        let bv = _mm256_loadu_ps(b.as_ptr().add(i));
        acc = _mm256_fmadd_ps(av, bv, acc);
        i += 8;
    }
    let mut sum = horizontal_sum(acc);
    for j in i..a.len() {
        sum += a[j] * b[j];
    }
    sum
}

/// Cosine distance: single loop accumulating dot + both norms.
///
/// # Safety
///
/// The caller must ensure `a` and `b` have the same length, and that the
/// current CPU supports AVX2 + FMA (checked by `is_x86_feature_detected!`
/// before dispatch).
#[target_feature(enable = "avx2,fma")]
pub unsafe fn distance_cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut acc_dot = _mm256_setzero_ps();
    let mut acc_na = _mm256_setzero_ps();
    let mut acc_nb = _mm256_setzero_ps();
    let mut i = 0;
    while i + 8 <= a.len() {
        let av = _mm256_loadu_ps(a.as_ptr().add(i));
        let bv = _mm256_loadu_ps(b.as_ptr().add(i));
        acc_dot = _mm256_fmadd_ps(av, bv, acc_dot);
        acc_na = _mm256_fmadd_ps(av, av, acc_na);
        acc_nb = _mm256_fmadd_ps(bv, bv, acc_nb);
        i += 8;
    }
    let mut dot = horizontal_sum(acc_dot);
    let mut norm_a = horizontal_sum(acc_na);
    let mut norm_b = horizontal_sum(acc_nb);
    for j in i..a.len() {
        dot += a[j] * b[j];
        norm_a += a[j] * a[j];
        norm_b += b[j] * b[j];
    }
    let denom = (norm_a * norm_b).sqrt();
    if denom == 0.0 {
        return 1.0;
    }
    1.0 - (dot / denom).clamp(-1.0, 1.0)
}

/// Dispatch for a metric.
///
/// # Safety
/// Must only be called with AVX2+FMA verified at runtime.
#[target_feature(enable = "avx2,fma")]
pub unsafe fn distance(metric: DistanceMetric, a: &[f32], b: &[f32]) -> f32 {
    match metric {
        DistanceMetric::Euclid => distance_l2(a, b),
        DistanceMetric::Dot => -inner_product(a, b),
        DistanceMetric::Cosine => distance_cosine(a, b),
        DistanceMetric::Manhattan => {
            // Intentionally scalar: Manhattan is rejected at collection
            // creation, kept here only for completeness.
            let mut sum = 0.0f32;
            for j in 0..a.len() {
                sum += (a[j] - b[j]).abs();
            }
            sum
        }
    }
}
