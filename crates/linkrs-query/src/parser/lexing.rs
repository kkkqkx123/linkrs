pub mod error;
pub mod inventory;
pub mod lexer;
pub mod spec;

pub use crate::parser::{Token, TokenKind};
pub use error::LexError;
pub use inventory::{
    function_category, keyword_table, KeywordEntry, MULTIWORD_CONDITIONAL, MULTIWORD_UNCONDITIONAL,
    SOFT_KEYWORDS,
};
pub use lexer::Lexer;
pub use lexer::LexerCheckpoint;
pub use spec::{
    check_symbol_token, is_identifier_spelling, operator_symbols, punctuation_symbols, symbol_kind,
    underscore_prop_kind, unit_spellings_for, KeywordCategory, SymbolClass, SymbolSpec,
    CONTEXTUAL_KEYWORDS, SYMBOL_SPEC, UNDERSCORE_PROPS,
};
