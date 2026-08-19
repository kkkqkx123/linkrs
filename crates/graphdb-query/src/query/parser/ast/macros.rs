//! Statement helper macro definitions
//!
//! Table-driven generation of `Stmt::span()` / `Stmt::category()` from a
//! single `(variant => category)` table, mirroring the plan-node macro
//! (`define_all_plan_nodes!`).

/// Generate `span()` and `category()` for `Stmt` from a single exhaustive
/// table.
///
/// Every variant carries a `span` field and belongs to exactly one
/// [`StmtCategory`], so both methods are uniform across all variants.
/// `Stmt::kind()` stays hand-written because three variants
/// (`Update.is_upsert`, `BeginTransaction.read_only`,
/// `RollbackTransaction.savepoint_name`) produce conditional strings that a
/// plain table cannot express.
#[macro_export]
macro_rules! define_stmt_helpers {
    ($($variant:ident => $category:ident),+ $(,)?) => {
        impl Stmt {
            pub fn span(&self) -> Span {
                match self {
                    $( Stmt::$variant(s) => s.span, )+
                }
            }

            pub fn category(&self) -> StmtCategory {
                use StmtCategory::*;
                match self {
                    $( Stmt::$variant(_) => $category, )+
                }
            }
        }
    };
}
