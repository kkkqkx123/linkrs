use crate::cursor::column_batch::ColumnValues;
use std::sync::Arc;

/// Predicate understood by native index cursors.
#[derive(Debug, Clone, PartialEq)]
pub enum IndexPredicate {
    Equal(linkrs_core::Value),
    Range {
        lower: Option<linkrs_core::Value>,
        upper: Option<linkrs_core::Value>,
        include_lower: bool,
        include_upper: bool,
    },
    Prefix(linkrs_core::Value),
    All,
}

/// A single-column comparison predicate pushed from the query layer into a
/// physical scan.
///
/// A list of predicates forms a conjunction (every predicate must match).
/// NULL rows follow the query engine's ordering: NULL sorts below every
/// scalar, so an upper-only interval also matches NULL. The original filter
/// expression still runs on top of the scan.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanPredicate {
    /// `column = value`
    ColumnEqual {
        column: Arc<str>,
        value: linkrs_core::Value,
    },
    /// `column` bounded by constants (either bound may be absent).
    ColumnRange {
        column: Arc<str>,
        lower: Option<linkrs_core::Value>,
        upper: Option<linkrs_core::Value>,
        include_lower: bool,
        include_upper: bool,
    },
}

impl ScanPredicate {
    /// Whether the predicate matches the given property set.
    ///
    /// Properties are a `(name, value)` slice in projection order.  A
    /// missing column (or any non-scalar comparison) never matches.
    pub fn matches(&self, props: &[(std::sync::Arc<str>, linkrs_core::Value)]) -> bool {
        let Some(value) = props
            .iter()
            .find(|(name, _)| name.as_ref() == self.column())
            .map(|(_, v)| v)
        else {
            return self.matches_null();
        };
        match self {
            ScanPredicate::ColumnEqual {
                value: expected, ..
            } => value == expected,
            ScanPredicate::ColumnRange {
                lower,
                upper,
                include_lower,
                include_upper,
                ..
            } => {
                if let Some(lower) = lower {
                    let ord = compare_scalar(value, lower);
                    let passes = if *include_lower {
                        ord != std::cmp::Ordering::Less
                    } else {
                        ord == std::cmp::Ordering::Greater
                    };
                    if !passes {
                        return false;
                    }
                }
                if let Some(upper) = upper {
                    let ord = compare_scalar(value, upper);
                    let passes = if *include_upper {
                        ord != std::cmp::Ordering::Greater
                    } else {
                        ord == std::cmp::Ordering::Less
                    };
                    if !passes {
                        return false;
                    }
                }
                true
            }
        }
    }

    /// Whether a NULL property satisfies this predicate.
    ///
    /// An upper-only range is the normalized form of `<` and `<=`. NULL
    /// sorts below every scalar literal, so those comparisons select NULL;
    /// equality and lower-bounded ranges reject it.
    pub fn matches_null(&self) -> bool {
        matches!(
            self,
            ScanPredicate::ColumnRange {
                lower: None,
                upper: Some(_),
                ..
            }
        )
    }

    /// The property column this predicate compares.
    pub fn column(&self) -> &str {
        match self {
            ScanPredicate::ColumnEqual { column, .. } => column,
            ScanPredicate::ColumnRange { column, .. } => column,
        }
    }

    pub fn column_arc(&self) -> Arc<str> {
        match self {
            ScanPredicate::ColumnEqual { column, .. } => column.clone(),
            ScanPredicate::ColumnRange { column, .. } => column.clone(),
        }
    }

    /// Merge pushed predicates into one value range per referenced column.
    ///
    /// The result is the conjunction of all predicates on that column:
    /// bounds only tighten. Columns compared with mixed orderings collapse
    /// into a single `[lower, upper]` interval; an equality contributes an
    /// empty interval.
    pub fn merged_ranges(predicates: &[ScanPredicate]) -> Vec<PredicateRange> {
        let mut ranges: Vec<PredicateRange> = Vec::new();
        for predicate in predicates {
            let (lower, include_lower, upper, include_upper) = match predicate {
                ScanPredicate::ColumnEqual { value, .. } => {
                    (Some(value.clone()), true, Some(value.clone()), true)
                }
                ScanPredicate::ColumnRange {
                    lower,
                    include_lower,
                    upper,
                    include_upper,
                    ..
                } => (lower.clone(), *include_lower, upper.clone(), *include_upper),
            };
            let column = predicate.column_arc();
            match ranges.iter_mut().find(|r| r.column == column) {
                Some(range) => range.intersect(lower, include_lower, upper, include_upper),
                None => ranges.push(PredicateRange {
                    column,
                    lower,
                    include_lower,
                    upper,
                    include_upper,
                }),
            }
        }
        ranges
    }
}

/// A merged value range over one column, derived from pushed scan
/// predicates. Used for zone-map pruning: a storage chunk whose min/max
/// bounds cannot overlap this range cannot contain matching rows.
#[derive(Debug, Clone)]
pub struct PredicateRange {
    pub column: Arc<str>,
    pub lower: Option<linkrs_core::Value>,
    pub include_lower: bool,
    pub upper: Option<linkrs_core::Value>,
    pub include_upper: bool,
}

impl PredicateRange {
    /// Tighten this range with another conjunctive bound set.
    fn intersect(
        &mut self,
        lower: Option<linkrs_core::Value>,
        include_lower: bool,
        upper: Option<linkrs_core::Value>,
        include_upper: bool,
    ) {
        if let (_, Some(new_lower)) = (&self.lower, &lower) {
            let replace = match &self.lower {
                Some(old) => match compare_scalar(old, new_lower) {
                    std::cmp::Ordering::Equal => self.include_lower && !include_lower,
                    std::cmp::Ordering::Less => true,
                    std::cmp::Ordering::Greater => false,
                },
                None => true,
            };
            if replace {
                self.lower = Some(new_lower.clone());
                self.include_lower = include_lower;
            }
        }
        if let (_, Some(new_upper)) = (&self.upper, &upper) {
            let replace = match &self.upper {
                Some(old) => match compare_scalar(old, new_upper) {
                    std::cmp::Ordering::Equal => self.include_upper && !include_upper,
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Less => false,
                },
                None => true,
            };
            if replace {
                self.upper = Some(new_upper.clone());
                self.include_upper = include_upper;
            }
        }
    }

    /// Whether a chunk holding values in `[min, max]` may contain rows
    /// matching this range. Conservative: returns `true` unless the chunk
    /// provably lies entirely outside.
    pub fn overlaps(&self, min: &linkrs_core::Value, max: &linkrs_core::Value) -> bool {
        // An upper-only interval also matches NULL rows, which the non-null
        // zone bounds cannot describe. Keep the chunk live for pruning.
        if self.lower.is_none() && self.upper.is_some() {
            return true;
        }
        if let Some(ref lower) = self.lower {
            // Chunk max must reach the query lower bound.
            let ord = compare_scalar(max, lower);
            let reaches = if self.include_lower {
                ord != std::cmp::Ordering::Less
            } else {
                ord == std::cmp::Ordering::Greater
            };
            if !reaches {
                return false;
            }
        }
        if let Some(ref upper) = self.upper {
            // Chunk min must not exceed the query upper bound.
            let ord = compare_scalar(min, upper);
            let under = if self.include_upper {
                ord != std::cmp::Ordering::Greater
            } else {
                ord == std::cmp::Ordering::Less
            };
            if !under {
                return false;
            }
        }
        true
    }

    /// Length of the equality probe when this range is a closed point
    /// (`lower == upper`, both inclusive). Used for length pre-pruning of
    /// complex values: a chunk whose length interval excludes the probe
    /// length cannot contain a match. Returns `None` for open ranges and
    /// for values without a prunable length.
    pub fn equality_len(&self) -> Option<usize> {
        if !self.include_lower || !self.include_upper {
            return None;
        }
        let lower = self.lower.as_ref()?;
        let upper = self.upper.as_ref()?;
        if compare_scalar(lower, upper) != std::cmp::Ordering::Equal {
            return None;
        }
        crate::vertex::column_store::complex_len(lower)
    }

    /// Scalar-leaf interval of the equality probe, if closed point over a
    /// leaf-carrying complex value. A chunk whose leaf interval is disjoint
    /// cannot contain a match even when outer lengths coincide. Uses the
    /// same flattening as chunk summaries, so exactly equal pairs share
    /// the interval.
    pub fn equality_leaf_range(&self) -> Option<(linkrs_core::Value, linkrs_core::Value)> {
        if !self.include_lower || !self.include_upper {
            return None;
        }
        let lower = self.lower.as_ref()?;
        let upper = self.upper.as_ref()?;
        if compare_scalar(lower, upper) != std::cmp::Ordering::Equal {
            return None;
        }
        crate::vertex::column_store::complex_leaf_range(lower)
    }

    /// Key fingerprint of the equality probe, if it carries keyed content.
    /// Returns `None` for keyless probes so the bloom gate stays disabled.
    pub fn equality_key_fp(&self) -> Option<u64> {
        if !self.include_lower || !self.include_upper {
            return None;
        }
        let lower = self.lower.as_ref()?;
        let upper = self.upper.as_ref()?;
        if compare_scalar(lower, upper) != std::cmp::Ordering::Equal {
            return None;
        }
        let fp = crate::vertex::column_store::complex_key_fp(lower);
        (fp != 0).then_some(fp)
    }
}

impl ScanPredicate {
    /// Columnar variant of [`matches`](Self::matches): evaluate against one
    /// [`ColumnValues`] at row `idx`. A null cell follows the same ordering
    /// as a NULL property in the row path.
    pub fn matches_column(&self, column: &ColumnValues, idx: usize) -> bool {
        match column.value_at(idx) {
            Some(value) => self.matches_scalar(&value),
            None => self.matches_null(),
        }
    }

    /// Evaluate the predicate against a single decoded value.
    ///
    /// Column-scan pushdown entry point: the caller already selected the
    /// predicate column, so no property-set lookup happens here.
    pub fn matches_value(&self, value: &linkrs_core::Value) -> bool {
        if matches!(value, linkrs_core::Value::Null(_)) {
            return self.matches_null();
        }
        self.matches_scalar(value)
    }

    /// Evaluate the predicate against a single decoded value.
    fn matches_scalar(&self, value: &linkrs_core::Value) -> bool {
        match self {
            ScanPredicate::ColumnEqual {
                value: expected, ..
            } => value == expected,
            ScanPredicate::ColumnRange {
                lower,
                upper,
                include_lower,
                include_upper,
                ..
            } => {
                if let Some(lower) = lower {
                    let ord = compare_scalar(value, lower);
                    let passes = if *include_lower {
                        ord != std::cmp::Ordering::Less
                    } else {
                        ord == std::cmp::Ordering::Greater
                    };
                    if !passes {
                        return false;
                    }
                }
                if let Some(upper) = upper {
                    let ord = compare_scalar(value, upper);
                    let passes = if *include_upper {
                        ord != std::cmp::Ordering::Greater
                    } else {
                        ord == std::cmp::Ordering::Less
                    };
                    if !passes {
                        return false;
                    }
                }
                true
            }
        }
    }
}

/// Compare two scalar values for a pushed predicate.
///
/// Integer kinds are compared exactly as `i64`; any numeric pair involving a
/// float is compared as `f64` (mirroring the query engine's typed batch
/// evaluation); everything else falls back to `Value` ordering.
///
/// Shared by segment-statistics widening so bounds merging uses exactly the
/// same ordering as pruning decisions.
pub fn compare_stat_values(a: &linkrs_core::Value, b: &linkrs_core::Value) -> std::cmp::Ordering {
    compare_scalar(a, b)
}

/// Compare two values exactly as the expression evaluator does.
///
/// Pushed predicates are only a pre-filter: the original expression still
/// runs on top of the scan. Using the same ordering as the evaluator keeps
/// the pre-filter conservative for every scalar kind, including NaN and
/// cross-kind numerics.
fn compare_scalar(a: &linkrs_core::Value, b: &linkrs_core::Value) -> std::cmp::Ordering {
    linkrs_core::Value::cmp(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use linkrs_core::value::NullType;

    fn upper_only() -> ScanPredicate {
        ScanPredicate::ColumnRange {
            column: "score".into(),
            lower: None,
            upper: Some(linkrs_core::Value::Int(15)),
            include_lower: false,
            include_upper: false,
        }
    }

    #[test]
    fn upper_only_range_selects_null_and_missing_properties() {
        let predicate = upper_only();
        let null = (Arc::from("score"), linkrs_core::Value::Null(NullType::Null));
        assert!(predicate.matches(std::slice::from_ref(&null)));
        assert!(predicate.matches(&[]));
        assert!(predicate.matches_value(&linkrs_core::Value::Null(NullType::Null)));
        assert!(predicate.matches_value(&linkrs_core::Value::Int(10)));
        assert!(!predicate.matches_value(&linkrs_core::Value::Int(20)));
    }

    #[test]
    fn equality_and_lower_bounded_ranges_reject_null() {
        let equal = ScanPredicate::ColumnEqual {
            column: "score".into(),
            value: linkrs_core::Value::Int(15),
        };
        let lower = ScanPredicate::ColumnRange {
            column: "score".into(),
            lower: Some(linkrs_core::Value::Int(15)),
            upper: None,
            include_lower: false,
            include_upper: false,
        };
        let null = linkrs_core::Value::Null(NullType::Null);
        assert!(!equal.matches_value(&null));
        assert!(!lower.matches_value(&null));
    }

    #[test]
    fn upper_only_range_is_not_zone_pruned() {
        let predicate = upper_only();
        let range = ScanPredicate::merged_ranges(std::slice::from_ref(&predicate)).remove(0);
        let min = linkrs_core::Value::Int(100);
        let max = linkrs_core::Value::Int(200);
        assert!(range.overlaps(&min, &max));
    }
}
