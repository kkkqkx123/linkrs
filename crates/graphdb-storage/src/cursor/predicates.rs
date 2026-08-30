use crate::cursor::column_batch::ColumnValues;

/// Predicate understood by native index cursors.
#[derive(Debug, Clone, PartialEq)]
pub enum IndexPredicate {
    Equal(graphdb_core::Value),
    Range {
        lower: Option<graphdb_core::Value>,
        upper: Option<graphdb_core::Value>,
        include_lower: bool,
        include_upper: bool,
    },
    Prefix(graphdb_core::Value),
    All,
}

/// A single-column comparison predicate pushed from the query layer into a
/// physical scan.
///
/// This is the whitelist of filter conjuncts the planner can push into the
/// storage layer.  A list of predicates forms a conjunction (every predicate
/// must match).  Rows with a missing property never match, mirroring the
/// query engine's NULL semantics where comparisons against NULL are false.
/// The original filter expression still runs on top of the scan, so the
/// pushdown is a pure pre-filter and can never change results.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanPredicate {
    /// `column = value`
    ColumnEqual {
        column: String,
        value: graphdb_core::Value,
    },
    /// `column` bounded by constants (either bound may be absent).
    ColumnRange {
        column: String,
        lower: Option<graphdb_core::Value>,
        upper: Option<graphdb_core::Value>,
        include_lower: bool,
        include_upper: bool,
    },
}

impl ScanPredicate {
    /// Whether the predicate matches the given property set.
    ///
    /// Properties are a `(name, value)` slice in projection order.  A
    /// missing column (or any non-scalar comparison) never matches.
    pub fn matches(&self, props: &[(String, graphdb_core::Value)]) -> bool {
        let Some(value) = props
            .iter()
            .find(|(name, _)| name == self.column())
            .map(|(_, v)| v)
        else {
            return false;
        };
        match self {
            ScanPredicate::ColumnEqual {
                value: expected, ..
            } => compare_scalar(value, expected) == std::cmp::Ordering::Equal,
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

    /// The property column this predicate compares.
    pub fn column(&self) -> &str {
        match self {
            ScanPredicate::ColumnEqual { column, .. } => column,
            ScanPredicate::ColumnRange { column, .. } => column,
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
            let column = predicate.column().to_string();
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
    pub column: String,
    pub lower: Option<graphdb_core::Value>,
    pub include_lower: bool,
    pub upper: Option<graphdb_core::Value>,
    pub include_upper: bool,
}

impl PredicateRange {
    /// Tighten this range with another conjunctive bound set.
    fn intersect(
        &mut self,
        lower: Option<graphdb_core::Value>,
        include_lower: bool,
        upper: Option<graphdb_core::Value>,
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
    pub fn overlaps(&self, min: &graphdb_core::Value, max: &graphdb_core::Value) -> bool {
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
}

impl ScanPredicate {
    /// Columnar variant of [`matches`](Self::matches): evaluate against one
    /// [`ColumnValues`] at row `idx`.  A null value never matches, mirroring
    /// the row-based NULL semantics.
    pub fn matches_column(&self, column: &ColumnValues, idx: usize) -> bool {
        let Some(value) = column.value_at(idx) else {
            return false;
        };
        self.matches_scalar(&value)
    }

    /// Evaluate the predicate against a single decoded value.
    fn matches_scalar(&self, value: &graphdb_core::Value) -> bool {
        match self {
            ScanPredicate::ColumnEqual {
                value: expected, ..
            } => compare_scalar(value, expected) == std::cmp::Ordering::Equal,
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
fn compare_scalar(a: &graphdb_core::Value, b: &graphdb_core::Value) -> std::cmp::Ordering {
    match (as_i64(a), as_i64(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => match (as_f64(a), as_f64(b)) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            _ => graphdb_core::Value::cmp(a, b),
        },
    }
}

fn as_i64(value: &graphdb_core::Value) -> Option<i64> {
    match value {
        graphdb_core::Value::SmallInt(v) => Some(*v as i64),
        graphdb_core::Value::Int(v) => Some(*v as i64),
        graphdb_core::Value::BigInt(v) => Some(*v),
        _ => None,
    }
}

fn as_f64(value: &graphdb_core::Value) -> Option<f64> {
    match value {
        graphdb_core::Value::Float(v) => Some(*v as f64),
        graphdb_core::Value::Double(v) => Some(*v),
        _ => None,
    }
}
