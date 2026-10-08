//! Immutable configuration for the N-way WCO intersect operator.
//!
//! One probe side plus N build sides share a single intersect variable.
//! Names (not slots) are stored so the operator resolves column positions
//! per input chunk, staying robust to upstream layout changes.

/// Immutable config for the WCO intersect operator.
#[derive(Debug, Clone)]
pub struct WcoSpec {
    /// Bound variable per build side, resolved both in probe rows (lookup)
    /// and in the matching build rows (table key). Length is the number of
    /// build sides and is always at least one.
    pub bound_names: Vec<String>,
    /// Intersect variable resolved in every build row and emitted into
    /// output rows.
    pub intersect_name: String,
    /// Output column order (the plan node's `col_names`).
    pub output_col_names: Vec<String>,
}

impl WcoSpec {
    /// Number of build sides.
    pub fn num_builds(&self) -> usize {
        self.bound_names.len()
    }
}
