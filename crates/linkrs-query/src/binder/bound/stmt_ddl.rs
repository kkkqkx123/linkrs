use crate::parser::ast::fulltext::{IndexFieldDef, IndexOptions};
use crate::parser::ast::vector::VectorIndexConfig;
use crate::parser::ast::{
    AlterTarget, DescTarget, DropTarget, ExplainFormat, ShowCreateTarget, ShowTarget,
};

use linkrs_core::types::semantic::{ColumnDef, ValueType};
use linkrs_core::types::FulltextEngineType;

use super::stmt::BoundStatement;

#[derive(Debug, Clone)]
pub struct BoundDrop {
    pub target: DropTarget,
    pub if_exists: bool,
}

#[derive(Debug, Clone)]
pub struct BoundAlter {
    pub target: AlterTarget,
}

#[derive(Debug, Clone)]
pub struct BoundShow {
    pub target: ShowTarget,
}

#[derive(Debug, Clone)]
pub struct BoundShowCreate {
    pub target: ShowCreateTarget,
}

#[derive(Debug, Clone)]
pub struct BoundDesc {
    pub target: DescTarget,
}

#[derive(Debug, Clone)]
pub struct BoundClearSpace {
    pub space_name: String,
}

#[derive(Debug, Clone)]
pub struct BoundCreateUser {
    pub username: String,
    pub password: String,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone)]
pub struct BoundDropUser {
    pub username: String,
    pub if_exists: bool,
}

#[derive(Debug, Clone)]
pub struct BoundAlterUser {
    pub username: String,
    pub password: Option<String>,
    pub is_locked: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct BoundCreateFulltextIndex {
    pub index_name: String,
    pub schema_name: String,
    pub fields: Vec<IndexFieldDef>,
    pub engine_type: FulltextEngineType,
    pub options: IndexOptions,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone)]
pub struct BoundCreateVectorIndex {
    pub index_name: String,
    pub schema_name: String,
    pub field_name: String,
    pub config: VectorIndexConfig,
    pub if_not_exists: bool,
}

#[derive(Debug, Clone)]
pub struct BoundExplain {
    pub statement: Box<BoundStatement>,
    pub format: ExplainFormat,
    pub analyze: bool,
}

#[derive(Debug, Clone)]
pub struct BoundProfile {
    pub statement: Box<BoundStatement>,
    pub format: ExplainFormat,
}

#[derive(Debug, Clone)]
pub struct BoundBeginTransaction {
    pub read_only: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct BoundCommit;

#[derive(Debug, Clone)]
pub struct BoundRollback {
    pub savepoint_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BoundSavepoint {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct BoundReleaseSavepoint {
    pub name: String,
}

/// Statement-level variable assignment (`$var = <query>`).
///
/// Unlike [`BoundAssignVariable`], which binds a single `LET` expression,
/// this carries a recursively bound inner statement for the planner to
/// plan via [`PlannerEnum::from_bound_statement`](crate::planning::planner::PlannerEnum::from_bound_statement).
#[derive(Debug, Clone)]
pub struct BoundAssignmentStatement {
    pub variable: String,
    pub statement: Box<BoundStatement>,
}

#[derive(Debug, Clone)]
pub struct BoundUse {
    pub space: String,
}

// ── ColumnDef helper ─────────────────────────────────────────────────────────

/// Column definition with name and type.
#[derive(Debug, Clone)]
pub struct BoundColumnDef {
    pub name: String,
    pub type_: ValueType,
}

impl From<ColumnDef> for BoundColumnDef {
    fn from(c: ColumnDef) -> Self {
        Self {
            name: c.name,
            type_: c.type_,
        }
    }
}
