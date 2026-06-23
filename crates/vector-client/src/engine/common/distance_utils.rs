//! Distance Calculation Utilities
//!
//! This module provides distance and similarity calculation functions
//! for various distance metrics that may not be directly supported by
//! the underlying vector database engine.
//!
//! # Qdrant Support
//!
//! Qdrant natively supports:
//! - Cosine distance
//! - Euclidean distance
//! - Dot product
//!
//! For other distance metrics (Manhattan, Hamming, Jaccard, Pearson),
//! custom implementation is required. These utilities can be used for:
//! - Pre-filtering vectors before storage
//! - Post-processing search results
//! - Custom scoring functions

/// Calculate Manhattan distance (L1 distance) between two vectors
///
/// Formula: sum(|a_i - b_i|)
pub fn manhattan_distance(v1: &[f32], v2: &[f32]) -> f32 {
    if v1.len() != v2.len() {
        panic!("Vector dimensions must match: {} vs {}", v1.len(), v2.len());
    }

    v1.iter().zip(v2.iter()).map(|(&a, &b)| (a - b).abs()).sum()
}

/// Calculate Chebyshev distance (L∞ distance) between two vectors
///
/// Formula: max(|a_i - b_i|)
pub fn chebyshev_distance(v1: &[f32], v2: &[f32]) -> f32 {
    if v1.len() != v2.len() {
        panic!("Vector dimensions must match: {} vs {}", v1.len(), v2.len());
    }

    v1.iter()
        .zip(v2.iter())
        .map(|(&a, &b)| (a - b).abs())
        .fold(0.0f32, f32::max)
}

/// Calculate Hamming distance between two binary vectors
///
/// Counts the number of positions where the corresponding bits differ.
/// Assumes binary vectors where values > 0.5 are considered 1, otherwise 0.
pub fn hamming_distance(v1: &[f32], v2: &[f32]) -> usize {
    if v1.len() != v2.len() {
        panic!("Vector dimensions must match: {} vs {}", v1.len(), v2.len());
    }

    v1.iter()
        .zip(v2.iter())
        .filter(|(&a, &b)| {
            let b1 = if a > 0.5 { 1 } else { 0 };
            let b2 = if b > 0.5 { 1 } else { 0 };
            b1 != b2
        })
        .count()
}

/// Calculate Jaccard similarity between two sets represented as binary vectors
///
/// Formula: |A ∩ B| / |A ∪ B|
/// Assumes binary vectors where values > 0.5 indicate membership in the set.
pub fn jaccard_similarity(set1: &[f32], set2: &[f32]) -> f32 {
    if set1.len() != set2.len() {
        panic!(
            "Vector dimensions must match: {} vs {}",
            set1.len(),
            set2.len()
        );
    }

    let mut intersection = 0;
    let mut union = 0;

    for (&a, &b) in set1.iter().zip(set2.iter()) {
        let in_set1 = a > 0.5;
        let in_set2 = b > 0.5;

        if in_set1 && in_set2 {
            intersection += 1;
        }

        if in_set1 || in_set2 {
            union += 1;
        }
    }

    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

/// Calculate Jaccard distance (1 - similarity)
pub fn jaccard_distance(set1: &[f32], set2: &[f32]) -> f32 {
    1.0 - jaccard_similarity(set1, set2)
}

/// Calculate Pearson correlation coefficient between two vectors
///
/// Formula: cov(X,Y) / (σ_X * σ_Y)
/// Returns a value in [-1, 1], where 1 is perfect positive correlation
pub fn pearson_correlation(v1: &[f32], v2: &[f32]) -> f32 {
    if v1.len() != v2.len() {
        panic!("Vector dimensions must match: {} vs {}", v1.len(), v2.len());
    }

    let n = v1.len() as f32;

    // Calculate sums
    let sum1: f32 = v1.iter().sum();
    let sum2: f32 = v2.iter().sum();

    // Calculate sum of squares
    let sum1_sq: f32 = v1.iter().map(|x| x * x).sum();
    let sum2_sq: f32 = v2.iter().map(|x| x * x).sum();

    // Calculate product sum
    let product_sum: f32 = v1.iter().zip(v2.iter()).map(|(&a, &b)| a * b).sum();

    // Calculate numerator and denominator
    let numerator = product_sum - (sum1 * sum2 / n);
    let denominator = ((sum1_sq - sum1.powi(2) / n) * (sum2_sq - sum2.powi(2) / n)).sqrt();

    if denominator == 0.0 {
        0.0
    } else {
        numerator / denominator
    }
}

/// Calculate Spearman rank correlation coefficient
///
/// Non-parametric measure of rank correlation.
pub fn spearman_correlation(v1: &[f32], v2: &[f32]) -> f32 {
    if v1.len() != v2.len() {
        panic!("Vector dimensions must match: {} vs {}", v1.len(), v2.len());
    }

    // Convert to ranks
    let ranks1 = compute_ranks(v1);
    let ranks2 = compute_ranks(v2);

    // Calculate Pearson correlation of ranks
    pearson_correlation(&ranks1, &ranks2)
}

/// Compute ranks for a vector
fn compute_ranks(values: &[f32]) -> Vec<f32> {
    let n = values.len();
    let mut indexed: Vec<(usize, f32)> = values.iter().copied().enumerate().collect();

    // Sort by value
    indexed.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    // Assign ranks (average ranks for ties)
    let mut ranks = vec![0.0f32; n];
    let mut i = 0;

    while i < n {
        let mut j = i;
        // Find all elements with the same value
        while j < n && indexed[j].1 == indexed[i].1 {
            j += 1;
        }

        // Average rank for tied values
        let avg_rank = (i as f32 + j as f32 + 1.0) / 2.0;

        for k in i..j {
            ranks[indexed[k].0] = avg_rank;
        }

        i = j;
    }

    ranks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manhattan_distance() {
        let v1 = vec![1.0, 2.0, 3.0];
        let v2 = vec![4.0, 5.0, 6.0];
        let distance = manhattan_distance(&v1, &v2);
        assert_eq!(distance, 9.0);
    }

    #[test]
    fn test_chebyshev_distance() {
        let v1 = vec![1.0, 5.0, 3.0];
        let v2 = vec![4.0, 2.0, 6.0];
        let distance = chebyshev_distance(&v1, &v2);
        assert_eq!(distance, 3.0);
    }

    #[test]
    fn test_hamming_distance() {
        let v1 = vec![1.0, 0.0, 1.0, 1.0];
        let v2 = vec![1.0, 1.0, 0.0, 1.0];
        let distance = hamming_distance(&v1, &v2);
        assert_eq!(distance, 2);
    }

    #[test]
    fn test_jaccard_similarity() {
        let set1 = vec![1.0, 1.0, 0.0, 0.0];
        let set2 = vec![1.0, 0.0, 1.0, 0.0];
        let similarity = jaccard_similarity(&set1, &set2);
        assert!((similarity - 0.333).abs() < 0.01);
    }

    #[test]
    fn test_pearson_correlation() {
        let v1 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let v2 = vec![2.0, 4.0, 6.0, 8.0, 10.0];
        let correlation = pearson_correlation(&v1, &v2);
        assert!((correlation - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_spearman_correlation() {
        let v1 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let v2 = vec![5.0, 4.0, 3.0, 2.0, 1.0];
        let correlation = spearman_correlation(&v1, &v2);
        assert!((correlation + 1.0).abs() < 0.001);
    }
}
