//! Parser module
//!
//! Responsible for parsing the top-level structure of query statements, including statements, expressions, patterns, etc.

mod clause_parser;
mod ddl_parser;
mod dml_parser;
mod expr_parser;
mod explain_parser;
mod fulltext_parser;
mod parse_context;
mod parser;
mod session_parser;
mod show_parser;
mod stmt_parser;
mod transaction_parser;
mod traversal_parser;
mod user_parser;
mod util_stmt_parser;
mod vector_parser;

#[cfg(test)]
mod tests;

pub use clause_parser::ClauseParser;
pub use ddl_parser::DdlParser;
pub use dml_parser::DmlParser;
pub use explain_parser::ExplainParser;
pub use fulltext_parser::parse_fulltext;
pub use parse_context::{ParseContext, RecoveryScope};
pub use parser::{Parser, ParserResult};
pub use session_parser::SessionParser;
pub use show_parser::ShowParser;
pub use stmt_parser::StmtParser;
pub use transaction_parser::TransactionParser;
pub use traversal_parser::TraversalParser;
pub use user_parser::UserParser;
pub use util_stmt_parser::UtilStmtParser;
pub use vector_parser::parse_vector;
