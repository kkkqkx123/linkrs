//! Degree sketch for skew-aware partition splitting.
//!
//! The sketch is a stride-sampled `(vertex id, out/in degree)` list sorted
//! by id, collected once per statistics pass and amortized over every later
//! compile. Partition boundaries cut at equal *cumulative* weight instead of
//! equal id width, so a degree-skewed anchor no longer pins the wall clock
//! on its tail partition. Weights are performance-only: any boundary choice
//! preserves row ownership and order semantics.

/// One stride sample: external vertex id with its total out/in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DegreeSketchSample {
    /// External vertex id (integer domains only).
    pub id: i64,
    /// Total out-degree across all edge types.
    pub out_degree: u64,
    /// Total in-degree across all edge types.
    pub in_degree: u64,
}

/// Cut `range` into `desired` sub-ranges at equal cumulative weight.
///
/// Samples must be sorted by id; the caller picks the out/in column through
/// `use_out`. Every sample id contributes `1 + degree` so the scan rows
/// themselves (uniform per row) stay balanced against the expand fan-out.
/// Degenerate input (fewer than two samples, all-zero weights, empty range)
/// falls back to `None` and the caller keeps the uniform split.
pub fn split_weighted_ranges(
    range: &std::ops::Range<i64>,
    desired: usize,
    samples: &[DegreeSketchSample],
    use_out: bool,
) -> Option<Vec<std::ops::Range<i64>>> {
    if desired < 2 || range.start >= range.end || samples.len() < 2 {
        return None;
    }
    let weights: Vec<u64> = samples
        .iter()
        .map(|sample| {
            1 + if use_out {
                sample.out_degree
            } else {
                sample.in_degree
            }
        })
        .collect();
    let total: u64 = weights.iter().sum();
    if total == 0 {
        return None;
    }
    let mut bounds = Vec::with_capacity(desired + 1);
    bounds.push(range.start);
    let mut cumulative = 0u64;
    let mut bucket = 1usize;
    for (index, sample) in samples.iter().enumerate() {
        cumulative += weights[index];
        while bucket < desired
            && cumulative.saturating_mul(desired as u64) >= total.saturating_mul(bucket as u64)
        {
            let cut = (sample.id + 1).clamp(range.start + 1, range.end);
            if cut > bounds[bucket - 1] {
                bounds.push(cut);
                bucket += 1;
            } else {
                break;
            }
        }
    }
    while bounds.len() <= desired {
        bounds.push(range.end);
    }
    bounds.truncate(desired + 1);
    if bounds.windows(2).any(|pair| pair[0] >= pair[1]) {
        return None;
    }
    Some(bounds.windows(2).map(|pair| pair[0]..pair[1]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: i64, out_degree: u64) -> DegreeSketchSample {
        DegreeSketchSample {
            id,
            out_degree,
            in_degree: 0,
        }
    }

    #[test]
    fn uniform_weights_match_uniform_split() {
        let samples: Vec<DegreeSketchSample> = (0..100).map(|id| sample(id, 1)).collect();
        let ranges = split_weighted_ranges(&(0..100), 4, &samples, true).unwrap();
        assert_eq!(ranges.len(), 4);
        assert_eq!(ranges[0].start, 0);
        assert_eq!(ranges[3].end, 100);
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
    }

    #[test]
    fn skewed_tail_moves_boundaries_left() {
        // First half is hot (degree 9), second half is cold (degree 0).
        // Equal cumulative weight must put more id width on the cold side.
        let mut samples = Vec::new();
        for id in 0..100 {
            samples.push(sample(id, if id < 50 { 9 } else { 0 }));
        }
        let weighted = split_weighted_ranges(&(0..100), 2, &samples, true).unwrap();
        let uniform_mid = 50;
        assert!(
            weighted[0].end < uniform_mid,
            "hot half must end before the uniform midpoint, got {:?}",
            weighted
        );
    }

    #[test]
    fn degenerate_input_falls_back() {
        assert!(split_weighted_ranges(&(0..100), 2, &[], true).is_none());
        let zeros: Vec<DegreeSketchSample> = (0..10).map(|id| sample(id, 0)).collect();
        // Weight is 1 + degree, so all-zero degrees still split.
        assert!(split_weighted_ranges(&(0..100), 2, &zeros, true).is_some());
        assert!(split_weighted_ranges(&(0..0), 2, &zeros, true).is_none());
        assert!(split_weighted_ranges(&(0..100), 1, &zeros, true).is_none());
    }

    #[test]
    fn single_sample_falls_back() {
        assert!(split_weighted_ranges(&(0..100), 2, &[sample(50, 5)], true).is_none());
    }
}
