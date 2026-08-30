use graphdb_core::types::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct MigratePlanStmt {
    pub span: Span,
    pub space: String,
    pub label: String,
    pub is_edge: bool,
    pub from_version: u64,
    pub to_version: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MigrateExecuteStmt {
    pub span: Span,
    pub plan_json: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MigrateRollbackStmt {
    pub span: Span,
    pub plan_json: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MigrateStmt {
    Plan(MigratePlanStmt),
    Execute(MigrateExecuteStmt),
    Rollback(MigrateRollbackStmt),
}

impl MigrateStmt {
    pub fn span(&self) -> Span {
        match self {
            MigrateStmt::Plan(s) => s.span,
            MigrateStmt::Execute(s) => s.span,
            MigrateStmt::Rollback(s) => s.span,
        }
    }
}
