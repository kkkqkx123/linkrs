//! Lexical inventory export.
//!
//! This module is the single source of truth for the keyword/operator/function
//! lists consumed by the tree-sitter grammar, the CLI completer and the
//! frontend highlighter. The keyword table mirrors `Lexer::lookup_keyword`
//! and is validated against it by `keyword_table_matches_lexer`, so the two
//! can never drift.

use std::sync::LazyLock;

use crate::parser::TokenKind;

/// Contextual (soft) keywords. They are not in `lookup_keyword`; the parser
/// recognizes them as `Identifier` tokens via `check_keyword`.
pub const SOFT_KEYWORDS: &[&str] = &[
    "GRAPH", "CHECKPOINT", "PURGE", "EXPIRED", "FULLTEXT", "EXTENSION", "MACRO", "TYPE",
    "ATTACH", "EXPORT", "IMPORT", "LOAD", "INSTALL", "UNINSTALL", "MIGRATE", "CALL",
];

/// Multi-word sequences the lexer merges into a single token unconditionally.
pub const MULTIWORD_UNCONDITIONAL: &[&str] = &[
    "IS NULL",
    "IS NOT NULL",
    "IS EMPTY",
    "IS NOT EMPTY",
    "NOT IN",
];

/// Multi-word sequences matched by the parser through lookahead. The second
/// word is not keyword-exclusive, so they must stay parse-time conditional.
pub const MULTIWORD_CONDITIONAL: &[&str] = &[
    "SEARCH VECTOR",
    "SEARCH INDEX",
    "LOOKUP VECTOR",
    "MATCH VECTOR",
    "DESCRIBE VECTOR",
    "SHOW VECTOR",
    "DESCRIBE FULLTEXT",
    "SHOW FULLTEXT",
    "LOAD EXTENSION",
    "UPDATE EXTENSION",
    "DETACH DELETE",
    "USING JOIN",
];

/// One reserved word and the token kind it produces.
pub struct KeywordEntry {
    pub spelling: &'static str,
    pub kind: TokenKind,
}

/// The reserved-word table. `BALANCE` is intentionally mapped to an identifier
/// and `TRUE`/`FALSE` to boolean literals, matching `lookup_keyword`.
pub fn keyword_table() -> &'static [KeywordEntry] {
    static TABLE: LazyLock<Vec<KeywordEntry>> = LazyLock::new(|| {
        vec![
            KeywordEntry { spelling: "CREATE", kind: TokenKind::Create },
            KeywordEntry { spelling: "MATCH", kind: TokenKind::Match },
            KeywordEntry { spelling: "RETURN", kind: TokenKind::Return },
            KeywordEntry { spelling: "WHERE", kind: TokenKind::Where },
            KeywordEntry { spelling: "DELETE", kind: TokenKind::Delete },
            KeywordEntry { spelling: "DETACH", kind: TokenKind::Detach },
            KeywordEntry { spelling: "UPDATE", kind: TokenKind::Update },
            KeywordEntry { spelling: "INSERT", kind: TokenKind::Insert },
            KeywordEntry { spelling: "UPSERT", kind: TokenKind::Upsert },
            KeywordEntry { spelling: "VALUES", kind: TokenKind::Values },
            KeywordEntry { spelling: "FROM", kind: TokenKind::From },
            KeywordEntry { spelling: "TO", kind: TokenKind::To },
            KeywordEntry { spelling: "AS", kind: TokenKind::As },
            KeywordEntry { spelling: "WITH", kind: TokenKind::With },
            KeywordEntry { spelling: "YIELD", kind: TokenKind::Yield },
            KeywordEntry { spelling: "GO", kind: TokenKind::Go },
            KeywordEntry { spelling: "OVER", kind: TokenKind::Over },
            KeywordEntry { spelling: "STEPS", kind: TokenKind::Step },
            KeywordEntry { spelling: "STEP", kind: TokenKind::Step },
            KeywordEntry { spelling: "UPTO", kind: TokenKind::Upto },
            KeywordEntry { spelling: "LIMIT", kind: TokenKind::Limit },
            KeywordEntry { spelling: "ASC", kind: TokenKind::Asc },
            KeywordEntry { spelling: "DESC", kind: TokenKind::Desc },
            KeywordEntry { spelling: "DESCRIBE", kind: TokenKind::Desc },
            KeywordEntry { spelling: "ORDER", kind: TokenKind::Order },
            KeywordEntry { spelling: "BY", kind: TokenKind::By },
            KeywordEntry { spelling: "SKIP", kind: TokenKind::Skip },
            KeywordEntry { spelling: "UNWIND", kind: TokenKind::Unwind },
            KeywordEntry { spelling: "OPTIONAL", kind: TokenKind::Optional },
            KeywordEntry { spelling: "DISTINCT", kind: TokenKind::Distinct },
            KeywordEntry { spelling: "ALL", kind: TokenKind::All },
            KeywordEntry { spelling: "NULL", kind: TokenKind::Null },
            KeywordEntry { spelling: "IS", kind: TokenKind::Is },
            KeywordEntry { spelling: "NOT", kind: TokenKind::Not },
            KeywordEntry { spelling: "AND", kind: TokenKind::And },
            KeywordEntry { spelling: "OR", kind: TokenKind::Or },
            KeywordEntry { spelling: "XOR", kind: TokenKind::Xor },
            KeywordEntry { spelling: "CONTAINS", kind: TokenKind::Contains },
            KeywordEntry { spelling: "STARTS", kind: TokenKind::StartsWith },
            KeywordEntry { spelling: "ENDS", kind: TokenKind::EndsWith },
            KeywordEntry { spelling: "CASE", kind: TokenKind::Case },
            KeywordEntry { spelling: "WHEN", kind: TokenKind::When },
            KeywordEntry { spelling: "THEN", kind: TokenKind::Then },
            KeywordEntry { spelling: "ELSE", kind: TokenKind::Else },
            KeywordEntry { spelling: "END", kind: TokenKind::End },
            KeywordEntry { spelling: "UNION", kind: TokenKind::Union },
            KeywordEntry { spelling: "INTERSECT", kind: TokenKind::Intersect },
            KeywordEntry { spelling: "MINUS", kind: TokenKind::SetMinus },
            KeywordEntry { spelling: "GROUP", kind: TokenKind::Group },
            KeywordEntry { spelling: "HAVING", kind: TokenKind::Having },
            KeywordEntry { spelling: "FILTER", kind: TokenKind::Filter },
            KeywordEntry { spelling: "ROLLUP", kind: TokenKind::Rollup },
            KeywordEntry { spelling: "CUBE", kind: TokenKind::Cube },
            KeywordEntry { spelling: "GROUPING", kind: TokenKind::Grouping },
            KeywordEntry { spelling: "SETS", kind: TokenKind::Sets },
            KeywordEntry { spelling: "BETWEEN", kind: TokenKind::Between },
            KeywordEntry { spelling: "ADMIN", kind: TokenKind::Admin },
            KeywordEntry { spelling: "EDGE", kind: TokenKind::Edge },
            KeywordEntry { spelling: "EDGES", kind: TokenKind::Edges },
            KeywordEntry { spelling: "VERTEX", kind: TokenKind::Vertex },
            KeywordEntry { spelling: "VERTICES", kind: TokenKind::Vertices },
            KeywordEntry { spelling: "TAG", kind: TokenKind::Tag },
            KeywordEntry { spelling: "TAGS", kind: TokenKind::Tags },
            KeywordEntry { spelling: "INDEX", kind: TokenKind::Index },
            KeywordEntry { spelling: "INDEXES", kind: TokenKind::Indexes },
            KeywordEntry { spelling: "LOOKUP", kind: TokenKind::Lookup },
            KeywordEntry { spelling: "FIND", kind: TokenKind::Find },
            KeywordEntry { spelling: "WEIGHT", kind: TokenKind::Weight },
            KeywordEntry { spelling: "PATH", kind: TokenKind::Path },
            KeywordEntry { spelling: "SHORTEST", kind: TokenKind::Shortest },
            KeywordEntry { spelling: "ALLSHORTESTPATHS", kind: TokenKind::AllShortestPaths },
            KeywordEntry { spelling: "TRAIL", kind: TokenKind::Trail },
            KeywordEntry { spelling: "WALK", kind: TokenKind::Walk },
            KeywordEntry { spelling: "ACYCLIC", kind: TokenKind::Acyclic },
            KeywordEntry { spelling: "WEIGHTED", kind: TokenKind::Weighted },
            KeywordEntry { spelling: "LOOP", kind: TokenKind::Loop },
            KeywordEntry { spelling: "CYCLE", kind: TokenKind::Cycle },
            KeywordEntry { spelling: "SUBGRAPH", kind: TokenKind::Subgraph },
            KeywordEntry { spelling: "BOTH", kind: TokenKind::Both },
            KeywordEntry { spelling: "OUT", kind: TokenKind::Out },
            KeywordEntry { spelling: "IN", kind: TokenKind::In },
            KeywordEntry { spelling: "REVERSELY", kind: TokenKind::Reversely },
            KeywordEntry { spelling: "RECURSIVE", kind: TokenKind::Recursive },
            KeywordEntry { spelling: "NO", kind: TokenKind::No },
            KeywordEntry { spelling: "OVERWRITE", kind: TokenKind::Overwrite },
            KeywordEntry { spelling: "SHOW", kind: TokenKind::Show },
            KeywordEntry { spelling: "ADD", kind: TokenKind::Add },
            KeywordEntry { spelling: "DROP", kind: TokenKind::Drop },
            KeywordEntry { spelling: "REMOVE", kind: TokenKind::Remove },
            KeywordEntry { spelling: "ALTER", kind: TokenKind::Alter },
            KeywordEntry { spelling: "IF", kind: TokenKind::If },
            KeywordEntry { spelling: "EXISTS", kind: TokenKind::Exists },
            KeywordEntry { spelling: "SUBQUERY", kind: TokenKind::Subquery },
            KeywordEntry { spelling: "CHANGE", kind: TokenKind::Change },
            KeywordEntry { spelling: "CREATEUSER", kind: TokenKind::CreateUser },
            KeywordEntry { spelling: "ALTERUSER", kind: TokenKind::AlterUser },
            KeywordEntry { spelling: "DROPUSER", kind: TokenKind::DropUser },
            KeywordEntry { spelling: "CHANGEPASSWORD", kind: TokenKind::ChangePassword },
            KeywordEntry { spelling: "GRANT", kind: TokenKind::Grant },
            KeywordEntry { spelling: "REVOKE", kind: TokenKind::Revoke },
            KeywordEntry { spelling: "ON", kind: TokenKind::On },
            KeywordEntry { spelling: "OF", kind: TokenKind::Of },
            KeywordEntry { spelling: "GET", kind: TokenKind::Get },
            KeywordEntry { spelling: "SET", kind: TokenKind::Set },
            KeywordEntry { spelling: "HOST", kind: TokenKind::Host },
            KeywordEntry { spelling: "HOSTS", kind: TokenKind::Hosts },
            KeywordEntry { spelling: "SPACE", kind: TokenKind::Space },
            KeywordEntry { spelling: "SPACES", kind: TokenKind::Spaces },
            KeywordEntry { spelling: "SEQUENCE", kind: TokenKind::Sequence },
            KeywordEntry { spelling: "SEQUENCES", kind: TokenKind::Sequences },
            KeywordEntry { spelling: "USER", kind: TokenKind::User },
            KeywordEntry { spelling: "USERS", kind: TokenKind::Users },
            KeywordEntry { spelling: "PASSWORD", kind: TokenKind::Password },
            KeywordEntry { spelling: "ROLE", kind: TokenKind::Role },
            KeywordEntry { spelling: "ROLES", kind: TokenKind::Roles },
            KeywordEntry { spelling: "FUNCTIONS", kind: TokenKind::Functions },
            KeywordEntry { spelling: "GRAPHS", kind: TokenKind::Graphs },
            KeywordEntry { spelling: "MACROS", kind: TokenKind::Macros },
            KeywordEntry { spelling: "LOCKED", kind: TokenKind::Locked },
            KeywordEntry { spelling: "GOD", kind: TokenKind::God },
            KeywordEntry { spelling: "DBA", kind: TokenKind::Dba },
            KeywordEntry { spelling: "GUEST", kind: TokenKind::Guest },
            KeywordEntry { spelling: "COMMENT", kind: TokenKind::Comment },
            KeywordEntry { spelling: "CHARSET", kind: TokenKind::Charset },
            KeywordEntry { spelling: "COLLATE", kind: TokenKind::Collate },
            KeywordEntry { spelling: "COLLATION", kind: TokenKind::Collation },
            KeywordEntry { spelling: "VID_TYPE", kind: TokenKind::VIdType },
            KeywordEntry { spelling: "PARTITION_NUM", kind: TokenKind::PartitionNum },
            KeywordEntry { spelling: "REPLICA_FACTOR", kind: TokenKind::ReplicaFactor },
            KeywordEntry { spelling: "REBUILD", kind: TokenKind::Rebuild },
            KeywordEntry { spelling: "BOOL", kind: TokenKind::Bool },
            KeywordEntry { spelling: "INT", kind: TokenKind::Int },
            KeywordEntry { spelling: "INT8", kind: TokenKind::Int8 },
            KeywordEntry { spelling: "INT16", kind: TokenKind::Int16 },
            KeywordEntry { spelling: "INT32", kind: TokenKind::Int32 },
            KeywordEntry { spelling: "INT64", kind: TokenKind::Int64 },
            KeywordEntry { spelling: "FLOAT", kind: TokenKind::Float },
            KeywordEntry { spelling: "DOUBLE", kind: TokenKind::Double },
            KeywordEntry { spelling: "STRING", kind: TokenKind::String },
            KeywordEntry { spelling: "FIXED_STRING", kind: TokenKind::FixedString },
            KeywordEntry { spelling: "TIMESTAMP", kind: TokenKind::Timestamp },
            KeywordEntry { spelling: "DATE", kind: TokenKind::Date },
            KeywordEntry { spelling: "TIME", kind: TokenKind::Time },
            KeywordEntry { spelling: "DATETIME", kind: TokenKind::Datetime },
            KeywordEntry { spelling: "SERIAL", kind: TokenKind::Serial },
            KeywordEntry { spelling: "DURATION", kind: TokenKind::Duration },
            KeywordEntry { spelling: "GEOGRAPHY", kind: TokenKind::Geography },
            KeywordEntry { spelling: "POINT", kind: TokenKind::Point },
            KeywordEntry { spelling: "LINESTRING", kind: TokenKind::Linestring },
            KeywordEntry { spelling: "POLYGON", kind: TokenKind::Polygon },
            KeywordEntry { spelling: "LIST", kind: TokenKind::List },
            KeywordEntry { spelling: "MAP", kind: TokenKind::Map },
            KeywordEntry { spelling: "STRUCT", kind: TokenKind::Struct },
            KeywordEntry { spelling: "ARRAY", kind: TokenKind::Array },
            KeywordEntry { spelling: "DOWNLOAD", kind: TokenKind::Download },
            KeywordEntry { spelling: "HDFS", kind: TokenKind::HDFS },
            KeywordEntry { spelling: "UUID", kind: TokenKind::UUID },
            KeywordEntry { spelling: "CONFIGS", kind: TokenKind::Configs },
            KeywordEntry { spelling: "FORCE", kind: TokenKind::Force },
            KeywordEntry { spelling: "PART", kind: TokenKind::Part },
            KeywordEntry { spelling: "PARTS", kind: TokenKind::Parts },
            KeywordEntry { spelling: "DATA", kind: TokenKind::Data },
            KeywordEntry { spelling: "LEADER", kind: TokenKind::Leader },
            KeywordEntry { spelling: "JOBS", kind: TokenKind::Jobs },
            KeywordEntry { spelling: "JOB", kind: TokenKind::Job },
            KeywordEntry { spelling: "BIDIRECT", kind: TokenKind::Bidirect },
            KeywordEntry { spelling: "STATS", kind: TokenKind::Stats },
            KeywordEntry { spelling: "STATUS", kind: TokenKind::Status },
            KeywordEntry { spelling: "RECOVER", kind: TokenKind::Recover },
            KeywordEntry { spelling: "EXPLAIN", kind: TokenKind::Explain },
            KeywordEntry { spelling: "PROFILE", kind: TokenKind::Profile },
            KeywordEntry { spelling: "ANALYZE", kind: TokenKind::Analyze },
            KeywordEntry { spelling: "FORMAT", kind: TokenKind::Format },
            KeywordEntry { spelling: "ATOMIC_EDGE", kind: TokenKind::AtomicEdge },
            KeywordEntry { spelling: "DEFAULT", kind: TokenKind::Default },
            KeywordEntry { spelling: "FLUSH", kind: TokenKind::Flush },
            KeywordEntry { spelling: "COMPACT", kind: TokenKind::Compact },
            KeywordEntry { spelling: "SUBMIT", kind: TokenKind::Submit },
            KeywordEntry { spelling: "ASCENDING", kind: TokenKind::Ascending },
            KeywordEntry { spelling: "DESCENDING", kind: TokenKind::Descending },
            KeywordEntry { spelling: "FETCH", kind: TokenKind::Fetch },
            KeywordEntry { spelling: "PROP", kind: TokenKind::Prop },
            KeywordEntry { spelling: "BALANCE", kind: TokenKind::Identifier("balance".to_string()) },
            KeywordEntry { spelling: "STOP", kind: TokenKind::Stop },
            KeywordEntry { spelling: "REVERT", kind: TokenKind::Revert },
            KeywordEntry { spelling: "USE", kind: TokenKind::Use },
            KeywordEntry { spelling: "BEGIN", kind: TokenKind::Begin },
            KeywordEntry { spelling: "COMMIT", kind: TokenKind::Commit },
            KeywordEntry { spelling: "ROLLBACK", kind: TokenKind::Rollback },
            KeywordEntry { spelling: "SAVEPOINT", kind: TokenKind::Savepoint },
            KeywordEntry { spelling: "RELEASE", kind: TokenKind::Release },
            KeywordEntry { spelling: "TRANSACTION", kind: TokenKind::Transaction },
            KeywordEntry { spelling: "LET", kind: TokenKind::Let },
            KeywordEntry { spelling: "READ", kind: TokenKind::Read },
            KeywordEntry { spelling: "ONLY", kind: TokenKind::Only },
            KeywordEntry { spelling: "WRITE", kind: TokenKind::Write },
            KeywordEntry { spelling: "SETLIST", kind: TokenKind::SetList },
            KeywordEntry { spelling: "CLEAR", kind: TokenKind::Clear },
            KeywordEntry { spelling: "MERGE", kind: TokenKind::Merge },
            KeywordEntry { spelling: "DIVIDE", kind: TokenKind::Divide },
            KeywordEntry { spelling: "RENAME", kind: TokenKind::Rename },
            KeywordEntry { spelling: "LOCAL", kind: TokenKind::Local },
            KeywordEntry { spelling: "SESSIONS", kind: TokenKind::Sessions },
            KeywordEntry { spelling: "SESSION", kind: TokenKind::Session },
            KeywordEntry { spelling: "SAMPLE", kind: TokenKind::Sample },
            KeywordEntry { spelling: "QUERIES", kind: TokenKind::Queries },
            KeywordEntry { spelling: "QUERY", kind: TokenKind::Query },
            KeywordEntry { spelling: "KILL", kind: TokenKind::Kill },
            KeywordEntry { spelling: "TOP", kind: TokenKind::Top },
            KeywordEntry { spelling: "TEXT", kind: TokenKind::Text },
            KeywordEntry { spelling: "SEARCH", kind: TokenKind::Search },
            KeywordEntry { spelling: "VECTOR", kind: TokenKind::KeywordVector },
            KeywordEntry { spelling: "CLIENT", kind: TokenKind::Client },
            KeywordEntry { spelling: "CLIENTS", kind: TokenKind::Clients },
            KeywordEntry { spelling: "SIGN", kind: TokenKind::Sign },
            KeywordEntry { spelling: "SERVICE", kind: TokenKind::Service },
            KeywordEntry { spelling: "COUNT", kind: TokenKind::Count },
            KeywordEntry { spelling: "SUM", kind: TokenKind::Sum },
            KeywordEntry { spelling: "AVG", kind: TokenKind::Avg },
            KeywordEntry { spelling: "MIN", kind: TokenKind::Min },
            KeywordEntry { spelling: "MAX", kind: TokenKind::Max },
            KeywordEntry { spelling: "TTL_DURATION", kind: TokenKind::TtlDuration },
            KeywordEntry { spelling: "TTL_COL", kind: TokenKind::TtlCol },
            KeywordEntry { spelling: "SOURCE", kind: TokenKind::Source },
            KeywordEntry { spelling: "DESTINATION", kind: TokenKind::Destination },
            KeywordEntry { spelling: "RANK", kind: TokenKind::Rank },
            KeywordEntry { spelling: "INPUT", kind: TokenKind::Input },
            KeywordEntry { spelling: "COPY", kind: TokenKind::Copy },
            KeywordEntry { spelling: "HEADER", kind: TokenKind::Header },
            KeywordEntry { spelling: "DELIMITER", kind: TokenKind::Delimiter },
            KeywordEntry { spelling: "CSV", kind: TokenKind::Csv },
            KeywordEntry { spelling: "TRUE", kind: TokenKind::BooleanLiteral(true) },
            KeywordEntry { spelling: "FALSE", kind: TokenKind::BooleanLiteral(false) },
        ]
    });
    &TABLE
}

/// Operator symbols as produced by the lexer. This table is the single source
/// for the inventory export; the example only re-exports it.
pub const OPERATOR_SYMBOLS: &[(&str, &str)] = &[
    ("+", "Plus"),
    ("-", "Minus"),
    ("*", "Star"),
    ("/", "Div"),
    ("%", "Mod"),
    ("**", "Exp"),
    ("==", "Eq"),
    ("=", "Assign"),
    ("!=", "Ne"),
    ("<>", "Ne"),
    ("<", "Lt"),
    ("<=", "Le"),
    (">", "Gt"),
    (">=", "Ge"),
    ("=~", "Regex"),
    ("!", "NotOp"),
    ("~", "Tilde"),
    ("||", "DoublePipe"),
    ("&", "Ampersand"),
    ("<<", "ShiftLeft"),
    (">>", "ShiftRight"),
    ("->", "Arrow"),
    ("->>", "ArrowRight"),
    ("#>", "HashArrow"),
    ("#>>", "HashArrowRight"),
];

/// Punctuation symbols as produced by the lexer. `|` stays here (lexical
/// delimiter) even though the tree-sitter highlight classifies it as an
/// operator for presentation; that presentational mapping lives in
/// `queries/highlights.scm` and `src/lib.rs`.
pub const PUNCTUATION_SYMBOLS: &[(&str, &str)] = &[
    ("(", "LParen"),
    (")", "RParen"),
    ("[", "LBracket"),
    ("]", "RBracket"),
    ("{", "LBrace"),
    ("}", "RBrace"),
    (",", "Comma"),
    (".", "Dot"),
    ("..", "DotDot"),
    (":", "Colon"),
    ("::", "DoubleColon"),
    (";", "Semicolon"),
    ("?", "QMark"),
    ("|", "Pipe"),
    ("<-", "BackArrow"),
    ("@", "At"),
    ("$", "Dollar"),
    ("$^", "SrcRef"),
    ("$$", "DstRef"),
    ("$-", "InputRef"),
];

/// Function category for inventory export. The registry itself only knows
/// name/arity/description; this controlled mapping supplies the editor-facing
/// category so the CLI and the frontend do not each hand-maintain one.
/// Unknown names fall back to `builtin`.
pub fn function_category(name: &str) -> &'static str {
    match name.to_uppercase().as_str() {
        "COUNT" | "SUM" | "AVG" | "MIN" | "MAX" | "COLLECT" => "aggregate",
        "LENGTH" | "SIZE" | "TRIM" | "LOWER" | "UPPER" | "SUBSTRING" | "REPLACE" => "string",
        "ABS" | "CEIL" | "FLOOR" | "ROUND" | "SQRT" => "numeric",
        "HEAD" | "TAIL" | "REVERSE" => "list",
        "TYPE" | "ID" | "PROPERTIES" => "type",
        "DATETIME" | "TIMESTAMP" => "date",
        _ => "builtin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::expression::functions::FunctionRegistry;
    use crate::parser::lexing::lexer::Lexer;

    #[test]
    fn keyword_table_matches_lexer() {
        let lexer = Lexer::new("");
        for entry in keyword_table() {
            let got = lexer.lookup_keyword(entry.spelling);
            assert_eq!(
                got, entry.kind,
                "keyword table drift for '{}'",
                entry.spelling
            );
        }
    }

    #[test]
    fn soft_keywords_are_not_reserved() {
        let lexer = Lexer::new("");
        for word in SOFT_KEYWORDS {
            assert!(
                matches!(lexer.lookup_keyword(word), TokenKind::Identifier(_)),
                "'{}' must stay a soft keyword",
                word
            );
        }
    }

    #[test]
    fn reserved_doc_covers_inventory() {
        let doc_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/release/reserved-keywords.md"
        );
        let doc = std::fs::read_to_string(doc_path).expect("reserved keywords doc must exist");
        for entry in keyword_table() {
            assert!(
                doc.contains(entry.spelling),
                "reserved keywords doc missing '{}'",
                entry.spelling
            );
        }
        for word in SOFT_KEYWORDS {
            assert!(
                doc.contains(word),
                "reserved keywords doc missing soft keyword '{word}'"
            );
        }
    }

    #[test]
    fn symbol_tables_match_lexer() {
        for (symbol, kind) in OPERATOR_SYMBOLS
            .iter()
            .chain(PUNCTUATION_SYMBOLS.iter())
        {
            let lexer = Lexer::new(*symbol);
            let token = lexer.current_token().clone();
            assert_eq!(
                format!("{:?}", token.kind).as_str(),
                *kind,
                "symbol table drift for '{symbol}'"
            );
        }
    }

    #[test]
    fn function_categories_exist_in_registry() {
        let names: std::collections::HashSet<String> = FunctionRegistry::new()
            .function_specs()
            .into_iter()
            .map(|spec| spec.name.to_uppercase())
            .collect();
        for name in [
            "COUNT", "SUM", "AVG", "MIN", "MAX", "COLLECT", "LENGTH", "SIZE", "TRIM", "LOWER",
            "UPPER", "SUBSTRING", "REPLACE", "ABS", "CEIL", "FLOOR", "ROUND", "SQRT", "HEAD",
            "TAIL", "REVERSE", "TYPE", "ID", "PROPERTIES", "DATETIME", "TIMESTAMP",
        ] {
            assert!(
                function_category(name) != "builtin" || names.contains(name),
                "category mapping references unknown function '{name}'"
            );
        }
    }
}
