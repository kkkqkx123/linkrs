//! Immutable configuration for apply operators.

use std::sync::Arc;

use crate::executor::streaming::plan::types::PhysicalPlan;
use graphdb_core::types::expr::Expression;

/// Immutable config for apply operators.
#[derive(Debug, Clone)]
pub enum ApplySpec {
    Apply {
        kind: ApplyKind,
        correlated_columns: Vec<String>,
    },
    PatternApply {
        hash_keys: Vec<Expression>,
        probe_keys: Vec<Expression>,
        anti: bool,
    },
    CorrelatedApply {
        /// Self-contained right subtree (rooted at an `Argument` source),
        /// re-executed once per outer row with the outer row bound as the
        /// correlation frame.
        sub_plan: Arc<PhysicalPlan>,
        anti: bool,
    },
    RollUpApply {
        compare_columns: Vec<String>,
        collect_column: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyKind {
    Standard,
    Semi,
    Anti,
    Single,
    All,
}
