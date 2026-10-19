//! Statement-level DML classification and notification.
//!
//! Graph databases have no per-row identity to report, so the classic
//! per-row update hook degrades to a statement-level notification: which
//! kind of write a statement performed, in which space, affecting how many
//! rows. The authoritative classifier is
//! [`crate::pipeline::prepared::dml_op_from_stmt`], derived from the parsed
//! [`Stmt`](crate::parser::ast::Stmt) during preparation; external hooks
//! must consume that (via `PreparedRequest::dml_op`) instead of scanning
//! text.
//!
//! The affected-rows count always comes from the result metadata and must be
//! treated as a row count, never as a stable row id.

use std::sync::Arc;

/// Kind of write a DML statement performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmlOp {
    Insert,
    Update,
    Delete,
}

/// Statement-level DML notification.
///
/// Emitted after a data-modifying statement succeeds. Valid only for the
/// dispatch call; observers must not retain it beyond the callback.
#[derive(Debug, Clone)]
pub struct DmlStatementEvent {
    pub op: DmlOp,
    pub space_name: String,
    pub rows: u64,
}

/// Runtime observer for statement-level DML notifications.
pub type DmlStatementCallback = Arc<dyn Fn(&DmlStatementEvent) + Send + Sync>;
