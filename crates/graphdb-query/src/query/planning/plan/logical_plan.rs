//! LogicalPlan: pure logical plan representation.
//!
//! Wraps a `LogicalNodeEnum` tree — pure logical operators with no
//! physical execution choices (IndexScan, InnerJoin, etc.).

use crate::query::planning::plan::logical::conversion::{convert_plan, ConversionError};
use crate::query::planning::plan::logical::LogicalNodeEnum;

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
#[derive(Debug, Clone)]
pub struct LogicalPlan {
    /// The root operator tree (pure logical operators only).
    pub root: LogicalNodeEnum,
    /// Statement kind.
    pub kind: StatementKind,
    /// Logical output schema: column names visible after this plan.
    pub output_column_names: Vec<String>,
}

impl LogicalPlan {
    pub fn new(root: LogicalNodeEnum) -> Self {
        let output_column_names = root.col_names().to_vec();
        Self {
            root,
            kind: StatementKind::Other,
            output_column_names,
        }
    }

    /// Build a `LogicalPlan` from a `PlanNodeEnum` tree by stripping
    /// physical execution choices.  Returns `ConversionError` when the
    /// plan contains a node type not yet handled by the converter.
    pub fn from_plan_node(
        plan: &crate::query::planning::plan::PlanNodeEnum,
    ) -> Result<Self, ConversionError> {
        let logical_root = convert_plan(plan)?;
        Ok(Self::new(logical_root))
    }

    pub fn with_kind(mut self, kind: StatementKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn root(&self) -> &LogicalNodeEnum {
        &self.root
    }

    pub fn into_root(self) -> LogicalNodeEnum {
        self.root
    }
}
