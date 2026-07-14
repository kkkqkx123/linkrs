//! LogicalPlan: first-phase logical plan representation.
//!
//! Phase 1 introduces a dedicated `LogicalPlan` type that wraps the
//! existing `PlanNodeEnum` tree and carries logical schema information
//! that is independent of physical execution choices.
//!
//! Current state (phase 1):
//! - `LogicalPlan` is a thin wrapper around `PlanNodeEnum` + metadata.
//! - The planner now returns both a `LogicalPlan` and an `ExecutionPlan`.
//! - Future phases will:
//!   1. Replace `PlanNodeEnum` variants with pure logical operators
//!   2. Move physical selection (index scan, hash join, etc.) to a
//!      physical conversion pass
//!   3. Have the optimizer consume `LogicalPlan` and produce `PhysicalPlan`

use crate::query::planning::plan::PlanNodeEnum;

/// Identifies the type of a statement that produced this plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    Match,
    Go,
    Lookup,
    Insert,
    Update,
    Delete,
    Create,
    Drop,
    Alter,
    Show,
    Search,
    Other,
}

/// Logical plan — pure operator tree with no physical execution choices.
///
/// Phase 1: wraps the existing PlanNodeEnum tree so that the planner
/// returns a typed logical plan.  Schema and semantic info will be
/// added in subsequent phases.
#[derive(Debug, Clone)]
pub struct LogicalPlan {
    /// The root operator tree (currently still PlanNodeEnum).
    pub root: PlanNodeEnum,
    /// Statement kind.
    pub kind: StatementKind,
    /// Logical output schema: column names visible after this plan.
    pub output_column_names: Vec<String>,
}

impl LogicalPlan {
    pub fn new(root: PlanNodeEnum) -> Self {
        let output_column_names = root.col_names().to_vec();
        Self {
            root,
            kind: StatementKind::Other,
            output_column_names,
        }
    }

    pub fn with_kind(mut self, kind: StatementKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn root(&self) -> &PlanNodeEnum {
        &self.root
    }

    pub fn into_root(self) -> PlanNodeEnum {
        self.root
    }
}
