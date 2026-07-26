//! Parser module
//!
//! Responsible for parsing the top-level structure of query statements, including statements, expressions, patterns, etc.

mod expr_parser;
mod fulltext_parser;
mod parse_context;
mod parser;
mod stmt_parser;
mod vector_parser;

// Sub-module parser
mod clause_parser;
mod ddl_parser;
mod dml_parser;
mod traversal_parser;
mod user_parser;
mod util_stmt_parser;

#[cfg(test)]
mod tests;

pub use expr_parser::ExprParser;
pub use fulltext_parser::parse_fulltext;
pub use parse_context::ParseContext;
pub use parser::{Parser, ParserResult};
pub use stmt_parser::StmtParser;
pub use vector_parser::parse_vector;

// Export Submodule Parser
pub use clause_parser::ClauseParser;
pub use ddl_parser::DdlParser;
pub use dml_parser::DmlParser;
pub use traversal_parser::TraversalParser;
pub use user_parser::UserParser;
pub use util_stmt_parser::UtilStmtParser;
