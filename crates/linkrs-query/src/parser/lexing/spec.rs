//! Single lexical specification for the query language.
//!
//! This module is the only place that states which spellings the language
//! has. The lexer (`lookup_keyword`), the parser (`check_keyword`,
//! `consume_token`), the `TokenKindExt` classification and the
//! `export_keyword_inventory` example all derive from these tables, so the
//! consumers cannot drift from each other: adding a spelling here reaches
//! every consumer, and there is no second hand-written list to forget.
//!
//! What lives here:
//!
//! - `keyword_table`: reserved words plus the payload spellings (`BALANCE`,
//!   `TRUE`/`FALSE`). Order is the long-standing export order so the
//!   regenerated `keywords.json` stays byte-identical.
//! - `KeywordCategory`: derived from the entry kind, never written per row.
//! - `CONTEXTUAL_KEYWORDS`: identifier spellings the parser recognizes by
//!   position (`check_keyword` on `Identifier`). They are not reserved and
//!   therefore stay out of the grammar export; they are listed here so the
//!   full parser vocabulary is enumerable in one place.
//! - `SYMBOL_SPEC`: every operator/punctuation spelling with its token kind
//!   and presentation class. The scanner still matches characters directly
//!   (lookahead is control flow, not a vocabulary list); `check_symbol_token`
//!   cross-checks scanner output against this table in test builds.
//! - `UNDERSCORE_PROPS`: the `_id` family of system property names.
//!
//! Deliberately out of scope (documented, not forgotten):
//!
//! - `expect_identifier` / `is_identifier_token` allow-lists: parse-acceptance
//!   policy (which reserved words may stand in as identifiers), not lexical
//!   truth. Changing them alters accepted syntax, so they stay with the
//!   parser.
//! - `function_category`: editor-facing presentation mapping with a `builtin`
//!   fallback, guarded by its own registry test.
//! - `grammar.js` SKIP/slug handling: build-chain presentation concern; the
//!   JSON contract is unchanged by this module.

use std::collections::{HashMap, HashSet};
use std::mem::Discriminant;
use std::sync::LazyLock;

use crate::parser::core::token::{TokenKind, TokenKindExt};

/// Category of a keyword-table entry, derived from the entry kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordCategory {
    /// Dedicated `TokenKind`, recognized unconditionally by the lexer.
    Reserved,
    /// `TRUE` / `FALSE`, producing `BooleanLiteral`.
    BooleanLiteral,
    /// `BALANCE`, producing a fixed `Identifier`.
    IdentifierMapped,
}

impl KeywordCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            KeywordCategory::Reserved => "reserved",
            KeywordCategory::BooleanLiteral => "boolean_literal",
            KeywordCategory::IdentifierMapped => "identifier_mapped",
        }
    }
}

/// One reserved word and the token kind it produces.
pub struct KeywordEntry {
    pub spelling: &'static str,
    pub kind: TokenKind,
}

impl KeywordEntry {
    pub fn category(&self) -> KeywordCategory {
        match &self.kind {
            TokenKind::BooleanLiteral(_) => KeywordCategory::BooleanLiteral,
            TokenKind::Identifier(_) => KeywordCategory::IdentifierMapped,
            _ => KeywordCategory::Reserved,
        }
    }
}

/// The reserved-word table. Order is the historical export order and must not
/// be reordered: the inventory export serializes it as-is.
pub fn keyword_table() -> &'static [KeywordEntry] {
    static TABLE: LazyLock<Vec<KeywordEntry>> = LazyLock::new(|| {
        vec![
            KeywordEntry {
                spelling: "CREATE",
                kind: TokenKind::Create,
            },
            KeywordEntry {
                spelling: "MATCH",
                kind: TokenKind::Match,
            },
            KeywordEntry {
                spelling: "RETURN",
                kind: TokenKind::Return,
            },
            KeywordEntry {
                spelling: "WHERE",
                kind: TokenKind::Where,
            },
            KeywordEntry {
                spelling: "DELETE",
                kind: TokenKind::Delete,
            },
            KeywordEntry {
                spelling: "DETACH",
                kind: TokenKind::Detach,
            },
            KeywordEntry {
                spelling: "UPDATE",
                kind: TokenKind::Update,
            },
            KeywordEntry {
                spelling: "INSERT",
                kind: TokenKind::Insert,
            },
            KeywordEntry {
                spelling: "UPSERT",
                kind: TokenKind::Upsert,
            },
            KeywordEntry {
                spelling: "VALUES",
                kind: TokenKind::Values,
            },
            KeywordEntry {
                spelling: "FROM",
                kind: TokenKind::From,
            },
            KeywordEntry {
                spelling: "TO",
                kind: TokenKind::To,
            },
            KeywordEntry {
                spelling: "AS",
                kind: TokenKind::As,
            },
            KeywordEntry {
                spelling: "WITH",
                kind: TokenKind::With,
            },
            KeywordEntry {
                spelling: "YIELD",
                kind: TokenKind::Yield,
            },
            KeywordEntry {
                spelling: "GO",
                kind: TokenKind::Go,
            },
            KeywordEntry {
                spelling: "OVER",
                kind: TokenKind::Over,
            },
            KeywordEntry {
                spelling: "STEPS",
                kind: TokenKind::Step,
            },
            KeywordEntry {
                spelling: "STEP",
                kind: TokenKind::Step,
            },
            KeywordEntry {
                spelling: "UPTO",
                kind: TokenKind::Upto,
            },
            KeywordEntry {
                spelling: "LIMIT",
                kind: TokenKind::Limit,
            },
            KeywordEntry {
                spelling: "ASC",
                kind: TokenKind::Asc,
            },
            KeywordEntry {
                spelling: "DESC",
                kind: TokenKind::Desc,
            },
            KeywordEntry {
                spelling: "DESCRIBE",
                kind: TokenKind::Desc,
            },
            KeywordEntry {
                spelling: "ORDER",
                kind: TokenKind::Order,
            },
            KeywordEntry {
                spelling: "BY",
                kind: TokenKind::By,
            },
            KeywordEntry {
                spelling: "SKIP",
                kind: TokenKind::Skip,
            },
            KeywordEntry {
                spelling: "UNWIND",
                kind: TokenKind::Unwind,
            },
            KeywordEntry {
                spelling: "OPTIONAL",
                kind: TokenKind::Optional,
            },
            KeywordEntry {
                spelling: "DISTINCT",
                kind: TokenKind::Distinct,
            },
            KeywordEntry {
                spelling: "ALL",
                kind: TokenKind::All,
            },
            KeywordEntry {
                spelling: "NULL",
                kind: TokenKind::Null,
            },
            KeywordEntry {
                spelling: "IS",
                kind: TokenKind::Is,
            },
            KeywordEntry {
                spelling: "NOT",
                kind: TokenKind::Not,
            },
            KeywordEntry {
                spelling: "AND",
                kind: TokenKind::And,
            },
            KeywordEntry {
                spelling: "OR",
                kind: TokenKind::Or,
            },
            KeywordEntry {
                spelling: "XOR",
                kind: TokenKind::Xor,
            },
            KeywordEntry {
                spelling: "CONTAINS",
                kind: TokenKind::Contains,
            },
            KeywordEntry {
                spelling: "STARTS",
                kind: TokenKind::StartsWith,
            },
            KeywordEntry {
                spelling: "ENDS",
                kind: TokenKind::EndsWith,
            },
            KeywordEntry {
                spelling: "CASE",
                kind: TokenKind::Case,
            },
            KeywordEntry {
                spelling: "WHEN",
                kind: TokenKind::When,
            },
            KeywordEntry {
                spelling: "THEN",
                kind: TokenKind::Then,
            },
            KeywordEntry {
                spelling: "ELSE",
                kind: TokenKind::Else,
            },
            KeywordEntry {
                spelling: "END",
                kind: TokenKind::End,
            },
            KeywordEntry {
                spelling: "UNION",
                kind: TokenKind::Union,
            },
            KeywordEntry {
                spelling: "INTERSECT",
                kind: TokenKind::Intersect,
            },
            KeywordEntry {
                spelling: "MINUS",
                kind: TokenKind::SetMinus,
            },
            KeywordEntry {
                spelling: "GROUP",
                kind: TokenKind::Group,
            },
            KeywordEntry {
                spelling: "HAVING",
                kind: TokenKind::Having,
            },
            KeywordEntry {
                spelling: "FILTER",
                kind: TokenKind::Filter,
            },
            KeywordEntry {
                spelling: "ROLLUP",
                kind: TokenKind::Rollup,
            },
            KeywordEntry {
                spelling: "CUBE",
                kind: TokenKind::Cube,
            },
            KeywordEntry {
                spelling: "GROUPING",
                kind: TokenKind::Grouping,
            },
            KeywordEntry {
                spelling: "SETS",
                kind: TokenKind::Sets,
            },
            KeywordEntry {
                spelling: "BETWEEN",
                kind: TokenKind::Between,
            },
            KeywordEntry {
                spelling: "ADMIN",
                kind: TokenKind::Admin,
            },
            KeywordEntry {
                spelling: "EDGE",
                kind: TokenKind::Edge,
            },
            KeywordEntry {
                spelling: "EDGES",
                kind: TokenKind::Edges,
            },
            KeywordEntry {
                spelling: "VERTEX",
                kind: TokenKind::Vertex,
            },
            KeywordEntry {
                spelling: "VERTICES",
                kind: TokenKind::Vertices,
            },
            KeywordEntry {
                spelling: "TAG",
                kind: TokenKind::Tag,
            },
            KeywordEntry {
                spelling: "TAGS",
                kind: TokenKind::Tags,
            },
            KeywordEntry {
                spelling: "INDEX",
                kind: TokenKind::Index,
            },
            KeywordEntry {
                spelling: "INDEXES",
                kind: TokenKind::Indexes,
            },
            KeywordEntry {
                spelling: "LOOKUP",
                kind: TokenKind::Lookup,
            },
            KeywordEntry {
                spelling: "FIND",
                kind: TokenKind::Find,
            },
            KeywordEntry {
                spelling: "WEIGHT",
                kind: TokenKind::Weight,
            },
            KeywordEntry {
                spelling: "PATH",
                kind: TokenKind::Path,
            },
            KeywordEntry {
                spelling: "SHORTEST",
                kind: TokenKind::Shortest,
            },
            KeywordEntry {
                spelling: "ALLSHORTESTPATHS",
                kind: TokenKind::AllShortestPaths,
            },
            KeywordEntry {
                spelling: "TRAIL",
                kind: TokenKind::Trail,
            },
            KeywordEntry {
                spelling: "WALK",
                kind: TokenKind::Walk,
            },
            KeywordEntry {
                spelling: "ACYCLIC",
                kind: TokenKind::Acyclic,
            },
            KeywordEntry {
                spelling: "WEIGHTED",
                kind: TokenKind::Weighted,
            },
            KeywordEntry {
                spelling: "LOOP",
                kind: TokenKind::Loop,
            },
            KeywordEntry {
                spelling: "CYCLE",
                kind: TokenKind::Cycle,
            },
            KeywordEntry {
                spelling: "SUBGRAPH",
                kind: TokenKind::Subgraph,
            },
            KeywordEntry {
                spelling: "BOTH",
                kind: TokenKind::Both,
            },
            KeywordEntry {
                spelling: "OUT",
                kind: TokenKind::Out,
            },
            KeywordEntry {
                spelling: "IN",
                kind: TokenKind::In,
            },
            KeywordEntry {
                spelling: "REVERSELY",
                kind: TokenKind::Reversely,
            },
            KeywordEntry {
                spelling: "RECURSIVE",
                kind: TokenKind::Recursive,
            },
            KeywordEntry {
                spelling: "NO",
                kind: TokenKind::No,
            },
            KeywordEntry {
                spelling: "OVERWRITE",
                kind: TokenKind::Overwrite,
            },
            KeywordEntry {
                spelling: "SHOW",
                kind: TokenKind::Show,
            },
            KeywordEntry {
                spelling: "ADD",
                kind: TokenKind::Add,
            },
            KeywordEntry {
                spelling: "DROP",
                kind: TokenKind::Drop,
            },
            KeywordEntry {
                spelling: "REMOVE",
                kind: TokenKind::Remove,
            },
            KeywordEntry {
                spelling: "ALTER",
                kind: TokenKind::Alter,
            },
            KeywordEntry {
                spelling: "IF",
                kind: TokenKind::If,
            },
            KeywordEntry {
                spelling: "EXISTS",
                kind: TokenKind::Exists,
            },
            KeywordEntry {
                spelling: "SUBQUERY",
                kind: TokenKind::Subquery,
            },
            KeywordEntry {
                spelling: "CHANGE",
                kind: TokenKind::Change,
            },
            KeywordEntry {
                spelling: "CREATEUSER",
                kind: TokenKind::CreateUser,
            },
            KeywordEntry {
                spelling: "ALTERUSER",
                kind: TokenKind::AlterUser,
            },
            KeywordEntry {
                spelling: "DROPUSER",
                kind: TokenKind::DropUser,
            },
            KeywordEntry {
                spelling: "CHANGEPASSWORD",
                kind: TokenKind::ChangePassword,
            },
            KeywordEntry {
                spelling: "GRANT",
                kind: TokenKind::Grant,
            },
            KeywordEntry {
                spelling: "REVOKE",
                kind: TokenKind::Revoke,
            },
            KeywordEntry {
                spelling: "ON",
                kind: TokenKind::On,
            },
            KeywordEntry {
                spelling: "OF",
                kind: TokenKind::Of,
            },
            KeywordEntry {
                spelling: "GET",
                kind: TokenKind::Get,
            },
            KeywordEntry {
                spelling: "SET",
                kind: TokenKind::Set,
            },
            KeywordEntry {
                spelling: "HOST",
                kind: TokenKind::Host,
            },
            KeywordEntry {
                spelling: "HOSTS",
                kind: TokenKind::Hosts,
            },
            KeywordEntry {
                spelling: "SPACE",
                kind: TokenKind::Space,
            },
            KeywordEntry {
                spelling: "SPACES",
                kind: TokenKind::Spaces,
            },
            KeywordEntry {
                spelling: "SEQUENCE",
                kind: TokenKind::Sequence,
            },
            KeywordEntry {
                spelling: "SEQUENCES",
                kind: TokenKind::Sequences,
            },
            KeywordEntry {
                spelling: "USER",
                kind: TokenKind::User,
            },
            KeywordEntry {
                spelling: "USERS",
                kind: TokenKind::Users,
            },
            KeywordEntry {
                spelling: "PASSWORD",
                kind: TokenKind::Password,
            },
            KeywordEntry {
                spelling: "ROLE",
                kind: TokenKind::Role,
            },
            KeywordEntry {
                spelling: "ROLES",
                kind: TokenKind::Roles,
            },
            KeywordEntry {
                spelling: "FUNCTIONS",
                kind: TokenKind::Functions,
            },
            KeywordEntry {
                spelling: "GRAPHS",
                kind: TokenKind::Graphs,
            },
            KeywordEntry {
                spelling: "MACROS",
                kind: TokenKind::Macros,
            },
            KeywordEntry {
                spelling: "LOCKED",
                kind: TokenKind::Locked,
            },
            KeywordEntry {
                spelling: "GOD",
                kind: TokenKind::God,
            },
            KeywordEntry {
                spelling: "DBA",
                kind: TokenKind::Dba,
            },
            KeywordEntry {
                spelling: "GUEST",
                kind: TokenKind::Guest,
            },
            KeywordEntry {
                spelling: "COMMENT",
                kind: TokenKind::Comment,
            },
            KeywordEntry {
                spelling: "CHARSET",
                kind: TokenKind::Charset,
            },
            KeywordEntry {
                spelling: "COLLATE",
                kind: TokenKind::Collate,
            },
            KeywordEntry {
                spelling: "COLLATION",
                kind: TokenKind::Collation,
            },
            KeywordEntry {
                spelling: "VID_TYPE",
                kind: TokenKind::VIdType,
            },
            KeywordEntry {
                spelling: "PARTITION_NUM",
                kind: TokenKind::PartitionNum,
            },
            KeywordEntry {
                spelling: "REPLICA_FACTOR",
                kind: TokenKind::ReplicaFactor,
            },
            KeywordEntry {
                spelling: "REBUILD",
                kind: TokenKind::Rebuild,
            },
            KeywordEntry {
                spelling: "BOOL",
                kind: TokenKind::Bool,
            },
            KeywordEntry {
                spelling: "INT",
                kind: TokenKind::Int,
            },
            KeywordEntry {
                spelling: "INT8",
                kind: TokenKind::Int8,
            },
            KeywordEntry {
                spelling: "INT16",
                kind: TokenKind::Int16,
            },
            KeywordEntry {
                spelling: "INT32",
                kind: TokenKind::Int32,
            },
            KeywordEntry {
                spelling: "INT64",
                kind: TokenKind::Int64,
            },
            KeywordEntry {
                spelling: "FLOAT",
                kind: TokenKind::Float,
            },
            KeywordEntry {
                spelling: "DOUBLE",
                kind: TokenKind::Double,
            },
            KeywordEntry {
                spelling: "STRING",
                kind: TokenKind::String,
            },
            KeywordEntry {
                spelling: "FIXED_STRING",
                kind: TokenKind::FixedString,
            },
            KeywordEntry {
                spelling: "TIMESTAMP",
                kind: TokenKind::Timestamp,
            },
            KeywordEntry {
                spelling: "DATE",
                kind: TokenKind::Date,
            },
            KeywordEntry {
                spelling: "TIME",
                kind: TokenKind::Time,
            },
            KeywordEntry {
                spelling: "DATETIME",
                kind: TokenKind::Datetime,
            },
            KeywordEntry {
                spelling: "SERIAL",
                kind: TokenKind::Serial,
            },
            KeywordEntry {
                spelling: "DURATION",
                kind: TokenKind::Duration,
            },
            KeywordEntry {
                spelling: "GEOGRAPHY",
                kind: TokenKind::Geography,
            },
            KeywordEntry {
                spelling: "POINT",
                kind: TokenKind::Point,
            },
            KeywordEntry {
                spelling: "LINESTRING",
                kind: TokenKind::Linestring,
            },
            KeywordEntry {
                spelling: "POLYGON",
                kind: TokenKind::Polygon,
            },
            KeywordEntry {
                spelling: "LIST",
                kind: TokenKind::List,
            },
            KeywordEntry {
                spelling: "MAP",
                kind: TokenKind::Map,
            },
            KeywordEntry {
                spelling: "STRUCT",
                kind: TokenKind::Struct,
            },
            KeywordEntry {
                spelling: "ARRAY",
                kind: TokenKind::Array,
            },
            KeywordEntry {
                spelling: "DOWNLOAD",
                kind: TokenKind::Download,
            },
            KeywordEntry {
                spelling: "HDFS",
                kind: TokenKind::HDFS,
            },
            KeywordEntry {
                spelling: "UUID",
                kind: TokenKind::UUID,
            },
            KeywordEntry {
                spelling: "CONFIGS",
                kind: TokenKind::Configs,
            },
            KeywordEntry {
                spelling: "FORCE",
                kind: TokenKind::Force,
            },
            KeywordEntry {
                spelling: "PART",
                kind: TokenKind::Part,
            },
            KeywordEntry {
                spelling: "PARTS",
                kind: TokenKind::Parts,
            },
            KeywordEntry {
                spelling: "DATA",
                kind: TokenKind::Data,
            },
            KeywordEntry {
                spelling: "LEADER",
                kind: TokenKind::Leader,
            },
            KeywordEntry {
                spelling: "JOBS",
                kind: TokenKind::Jobs,
            },
            KeywordEntry {
                spelling: "JOB",
                kind: TokenKind::Job,
            },
            KeywordEntry {
                spelling: "BIDIRECT",
                kind: TokenKind::Bidirect,
            },
            KeywordEntry {
                spelling: "STATS",
                kind: TokenKind::Stats,
            },
            KeywordEntry {
                spelling: "STATUS",
                kind: TokenKind::Status,
            },
            KeywordEntry {
                spelling: "RECOVER",
                kind: TokenKind::Recover,
            },
            KeywordEntry {
                spelling: "EXPLAIN",
                kind: TokenKind::Explain,
            },
            KeywordEntry {
                spelling: "PROFILE",
                kind: TokenKind::Profile,
            },
            KeywordEntry {
                spelling: "ANALYZE",
                kind: TokenKind::Analyze,
            },
            KeywordEntry {
                spelling: "FORMAT",
                kind: TokenKind::Format,
            },
            KeywordEntry {
                spelling: "ATOMIC_EDGE",
                kind: TokenKind::AtomicEdge,
            },
            KeywordEntry {
                spelling: "DEFAULT",
                kind: TokenKind::Default,
            },
            KeywordEntry {
                spelling: "FLUSH",
                kind: TokenKind::Flush,
            },
            KeywordEntry {
                spelling: "COMPACT",
                kind: TokenKind::Compact,
            },
            KeywordEntry {
                spelling: "SUBMIT",
                kind: TokenKind::Submit,
            },
            KeywordEntry {
                spelling: "ASCENDING",
                kind: TokenKind::Ascending,
            },
            KeywordEntry {
                spelling: "DESCENDING",
                kind: TokenKind::Descending,
            },
            KeywordEntry {
                spelling: "FETCH",
                kind: TokenKind::Fetch,
            },
            KeywordEntry {
                spelling: "PROP",
                kind: TokenKind::Prop,
            },
            KeywordEntry {
                spelling: "BALANCE",
                kind: TokenKind::Identifier("balance".to_string()),
            },
            KeywordEntry {
                spelling: "STOP",
                kind: TokenKind::Stop,
            },
            KeywordEntry {
                spelling: "REVERT",
                kind: TokenKind::Revert,
            },
            KeywordEntry {
                spelling: "USE",
                kind: TokenKind::Use,
            },
            KeywordEntry {
                spelling: "BEGIN",
                kind: TokenKind::Begin,
            },
            KeywordEntry {
                spelling: "COMMIT",
                kind: TokenKind::Commit,
            },
            KeywordEntry {
                spelling: "ROLLBACK",
                kind: TokenKind::Rollback,
            },
            KeywordEntry {
                spelling: "SAVEPOINT",
                kind: TokenKind::Savepoint,
            },
            KeywordEntry {
                spelling: "RELEASE",
                kind: TokenKind::Release,
            },
            KeywordEntry {
                spelling: "TRANSACTION",
                kind: TokenKind::Transaction,
            },
            KeywordEntry {
                spelling: "LET",
                kind: TokenKind::Let,
            },
            KeywordEntry {
                spelling: "READ",
                kind: TokenKind::Read,
            },
            KeywordEntry {
                spelling: "ONLY",
                kind: TokenKind::Only,
            },
            KeywordEntry {
                spelling: "WRITE",
                kind: TokenKind::Write,
            },
            KeywordEntry {
                spelling: "SETLIST",
                kind: TokenKind::SetList,
            },
            KeywordEntry {
                spelling: "CLEAR",
                kind: TokenKind::Clear,
            },
            KeywordEntry {
                spelling: "MERGE",
                kind: TokenKind::Merge,
            },
            KeywordEntry {
                spelling: "DIVIDE",
                kind: TokenKind::Divide,
            },
            KeywordEntry {
                spelling: "RENAME",
                kind: TokenKind::Rename,
            },
            KeywordEntry {
                spelling: "LOCAL",
                kind: TokenKind::Local,
            },
            KeywordEntry {
                spelling: "SESSIONS",
                kind: TokenKind::Sessions,
            },
            KeywordEntry {
                spelling: "SESSION",
                kind: TokenKind::Session,
            },
            KeywordEntry {
                spelling: "SAMPLE",
                kind: TokenKind::Sample,
            },
            KeywordEntry {
                spelling: "QUERIES",
                kind: TokenKind::Queries,
            },
            KeywordEntry {
                spelling: "QUERY",
                kind: TokenKind::Query,
            },
            KeywordEntry {
                spelling: "KILL",
                kind: TokenKind::Kill,
            },
            KeywordEntry {
                spelling: "TOP",
                kind: TokenKind::Top,
            },
            KeywordEntry {
                spelling: "TEXT",
                kind: TokenKind::Text,
            },
            KeywordEntry {
                spelling: "SEARCH",
                kind: TokenKind::Search,
            },
            KeywordEntry {
                spelling: "VECTOR",
                kind: TokenKind::KeywordVector,
            },
            KeywordEntry {
                spelling: "CLIENT",
                kind: TokenKind::Client,
            },
            KeywordEntry {
                spelling: "CLIENTS",
                kind: TokenKind::Clients,
            },
            KeywordEntry {
                spelling: "SIGN",
                kind: TokenKind::Sign,
            },
            KeywordEntry {
                spelling: "SERVICE",
                kind: TokenKind::Service,
            },
            KeywordEntry {
                spelling: "COUNT",
                kind: TokenKind::Count,
            },
            KeywordEntry {
                spelling: "SUM",
                kind: TokenKind::Sum,
            },
            KeywordEntry {
                spelling: "AVG",
                kind: TokenKind::Avg,
            },
            KeywordEntry {
                spelling: "MIN",
                kind: TokenKind::Min,
            },
            KeywordEntry {
                spelling: "MAX",
                kind: TokenKind::Max,
            },
            KeywordEntry {
                spelling: "TTL_DURATION",
                kind: TokenKind::TtlDuration,
            },
            KeywordEntry {
                spelling: "TTL_COL",
                kind: TokenKind::TtlCol,
            },
            KeywordEntry {
                spelling: "SOURCE",
                kind: TokenKind::Source,
            },
            KeywordEntry {
                spelling: "DESTINATION",
                kind: TokenKind::Destination,
            },
            KeywordEntry {
                spelling: "RANK",
                kind: TokenKind::Rank,
            },
            KeywordEntry {
                spelling: "INPUT",
                kind: TokenKind::Input,
            },
            KeywordEntry {
                spelling: "COPY",
                kind: TokenKind::Copy,
            },
            KeywordEntry {
                spelling: "HEADER",
                kind: TokenKind::Header,
            },
            KeywordEntry {
                spelling: "DELIMITER",
                kind: TokenKind::Delimiter,
            },
            KeywordEntry {
                spelling: "CSV",
                kind: TokenKind::Csv,
            },
            KeywordEntry {
                spelling: "TRUE",
                kind: TokenKind::BooleanLiteral(true),
            },
            KeywordEntry {
                spelling: "FALSE",
                kind: TokenKind::BooleanLiteral(false),
            },
        ]
    });
    &TABLE
}

/// Contextual keywords: `Identifier` spellings the parser recognizes by
/// position (statement, clause and subquery introducers). They are not
/// reserved, so they stay out of the grammar export, but they are part of the
/// parser vocabulary and therefore listed here instead of hiding inside
/// `check_keyword` call sites and ad-hoc identifier matches.
///
/// Out of scope by criterion: type-position aliases (`DECIMAL`) and option
/// values (`true`/`false` in `COPY` options) are value policy, not clause
/// vocabulary, and stay with their parsers.
pub const CONTEXTUAL_KEYWORDS: &[&str] = &[
    "GRAPH",
    "CHECKPOINT",
    "PURGE",
    "EXPIRED",
    "FULLTEXT",
    "EXTENSION",
    "MACRO",
    "TYPE",
    "ATTACH",
    "EXPORT",
    "IMPORT",
    "LOAD",
    "INSTALL",
    "UNINSTALL",
    "MIGRATE",
    "CALL",
    "COLLECT",
    "USING",
    "JOIN",
    "BINARY",
    "MULTIWAY",
    "ANALYZER",
    "ATTACHED",
    "BATCH",
    "BATCH_SIZE",
    "BOOST",
    "COLUMN",
    "DATABASE",
    "DATABASE_TYPE",
    "DATABASES",
    "DBTYPE",
    "ENGINE",
    "EXECUTE",
    "EXTENSIONS",
    "FIELD",
    "FOR",
    "FULLTEXT_MATCH",
    "GLOB",
    "HEADERS",
    "INCREMENT",
    "LIKE",
    "MAXVALUE",
    "MINVALUE",
    "NOCYCLE",
    "OFFSET",
    "OPTIMIZE",
    "OPTIONS",
    "PARTITION",
    "PLAN",
    "SELECT",
    "SIZE",
    "START",
    "THRESHOLD",
    "VERSION",
    "matched_fields",
    "score",
];

/// Soft keywords exported for completion and highlighting. This is the
/// grammar-compatible subset of `CONTEXTUAL_KEYWORDS`, kept stable so the
/// tree-sitter grammar and the frontend do not gain new reserved rules.
pub const SOFT_KEYWORDS: &[&str] = &[
    "GRAPH",
    "CHECKPOINT",
    "PURGE",
    "EXPIRED",
    "FULLTEXT",
    "EXTENSION",
    "MACRO",
    "TYPE",
    "ATTACH",
    "EXPORT",
    "IMPORT",
    "LOAD",
    "INSTALL",
    "UNINSTALL",
    "MIGRATE",
    "CALL",
    "COLLECT",
    "USING",
    "JOIN",
    "BINARY",
    "MULTIWAY",
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

/// Presentation class of a symbol: operator for evaluation, punctuation for
/// structure. `|` stays punctuation even though the tree-sitter highlight
/// classifies it as an operator for presentation; that mapping lives in
/// `queries/highlights.scm` and `src/lib.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolClass {
    Operator,
    Punctuation,
}

/// One symbol spelling and the token kind the lexer produces for it.
pub struct SymbolSpec {
    pub symbol: &'static str,
    pub kind: TokenKind,
    pub class: SymbolClass,
}

/// Every operator/punctuation spelling the lexer produces, in historical
/// export order. All kinds here are unit variants, so this table is `const`.
pub const SYMBOL_SPEC: &[SymbolSpec] = &[
    SymbolSpec {
        symbol: "+",
        kind: TokenKind::Plus,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "-",
        kind: TokenKind::Minus,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "*",
        kind: TokenKind::Star,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "/",
        kind: TokenKind::Div,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "%",
        kind: TokenKind::Mod,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "**",
        kind: TokenKind::Exp,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "==",
        kind: TokenKind::Eq,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "=",
        kind: TokenKind::Assign,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "!=",
        kind: TokenKind::Ne,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "<>",
        kind: TokenKind::Ne,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "<",
        kind: TokenKind::Lt,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "<=",
        kind: TokenKind::Le,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: ">",
        kind: TokenKind::Gt,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: ">=",
        kind: TokenKind::Ge,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "=~",
        kind: TokenKind::Regex,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "!",
        kind: TokenKind::NotOp,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "~",
        kind: TokenKind::Tilde,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "||",
        kind: TokenKind::DoublePipe,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "&",
        kind: TokenKind::Ampersand,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "<<",
        kind: TokenKind::ShiftLeft,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: ">>",
        kind: TokenKind::ShiftRight,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "->",
        kind: TokenKind::Arrow,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "->>",
        kind: TokenKind::ArrowRight,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "#>",
        kind: TokenKind::HashArrow,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "#>>",
        kind: TokenKind::HashArrowRight,
        class: SymbolClass::Operator,
    },
    SymbolSpec {
        symbol: "(",
        kind: TokenKind::LParen,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: ")",
        kind: TokenKind::RParen,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "[",
        kind: TokenKind::LBracket,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "]",
        kind: TokenKind::RBracket,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "{",
        kind: TokenKind::LBrace,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "}",
        kind: TokenKind::RBrace,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: ",",
        kind: TokenKind::Comma,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: ".",
        kind: TokenKind::Dot,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "..",
        kind: TokenKind::DotDot,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: ":",
        kind: TokenKind::Colon,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "::",
        kind: TokenKind::DoubleColon,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: ";",
        kind: TokenKind::Semicolon,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "?",
        kind: TokenKind::QMark,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "|",
        kind: TokenKind::Pipe,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "<-",
        kind: TokenKind::BackArrow,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "@",
        kind: TokenKind::At,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "$",
        kind: TokenKind::Dollar,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "$^",
        kind: TokenKind::SrcRef,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "$$",
        kind: TokenKind::DstRef,
        class: SymbolClass::Punctuation,
    },
    SymbolSpec {
        symbol: "$-",
        kind: TokenKind::InputRef,
        class: SymbolClass::Punctuation,
    },
];

/// Operator symbols in export order.
pub fn operator_symbols() -> Vec<(&'static str, TokenKind)> {
    SYMBOL_SPEC
        .iter()
        .filter(|entry| entry.class == SymbolClass::Operator)
        .map(|entry| (entry.symbol, entry.kind.clone()))
        .collect()
}

/// Punctuation symbols in export order.
pub fn punctuation_symbols() -> Vec<(&'static str, TokenKind)> {
    SYMBOL_SPEC
        .iter()
        .filter(|entry| entry.class == SymbolClass::Punctuation)
        .map(|entry| (entry.symbol, entry.kind.clone()))
        .collect()
}

/// System property names (`_id` family) and the token kinds they produce.
pub const UNDERSCORE_PROPS: &[(&str, TokenKind)] = &[
    ("_id", TokenKind::IdProp),
    ("_type", TokenKind::TypeProp),
    ("_src", TokenKind::SrcIdProp),
    ("_dst", TokenKind::DstIdProp),
    ("_rank", TokenKind::RankProp),
];

pub fn underscore_prop_kind(name: &str) -> Option<TokenKind> {
    UNDERSCORE_PROPS
        .iter()
        .find(|(spelling, _)| *spelling == name)
        .map(|(_, kind)| kind.clone())
}

struct SpecIndex {
    by_spelling: HashMap<&'static str, usize>,
    unit_spellings: HashMap<Discriminant<TokenKind>, Vec<&'static str>>,
    symbols: HashMap<&'static str, TokenKind>,
    symbol_lexemes: HashMap<Discriminant<TokenKind>, Vec<&'static str>>,
    reserved: HashSet<Discriminant<TokenKind>>,
    operators: HashSet<Discriminant<TokenKind>>,
    punctuation: HashSet<Discriminant<TokenKind>>,
}

fn discriminant(kind: &TokenKind) -> Discriminant<TokenKind> {
    std::mem::discriminant(kind)
}

static SPEC_INDEX: LazyLock<SpecIndex> = LazyLock::new(|| {
    let mut index = SpecIndex {
        by_spelling: HashMap::new(),
        unit_spellings: HashMap::new(),
        symbols: HashMap::new(),
        symbol_lexemes: HashMap::new(),
        reserved: HashSet::new(),
        operators: HashSet::new(),
        punctuation: HashSet::new(),
    };
    for (position, entry) in keyword_table().iter().enumerate() {
        index.by_spelling.insert(entry.spelling, position);
        if entry.category() == KeywordCategory::Reserved {
            let key = discriminant(&entry.kind);
            index
                .unit_spellings
                .entry(key)
                .or_default()
                .push(entry.spelling);
            index.reserved.insert(key);
        }
    }
    for entry in SYMBOL_SPEC {
        index.symbols.insert(entry.symbol, entry.kind.clone());
        let key = discriminant(&entry.kind);
        index
            .symbol_lexemes
            .entry(key)
            .or_default()
            .push(entry.symbol);
        match entry.class {
            SymbolClass::Operator => {
                index.operators.insert(key);
            }
            SymbolClass::Punctuation => {
                index.punctuation.insert(key);
            }
        }
    }
    index
});

/// Resolve an upper-cased identifier against the keyword table.
pub fn lookup_keyword_spelling(upper: &str) -> Option<TokenKind> {
    SPEC_INDEX
        .by_spelling
        .get(upper)
        .map(|position| keyword_table()[*position].kind.clone())
}

/// Canonical spellings for a reserved unit token kind, if any.
pub fn unit_spellings_for(kind: &TokenKind) -> Option<&'static [&'static str]> {
    SPEC_INDEX
        .unit_spellings
        .get(&discriminant(kind))
        .map(Vec::as_slice)
}

/// Resolve a symbol spelling to its token kind.
pub fn symbol_kind(symbol: &str) -> Option<TokenKind> {
    SPEC_INDEX.symbols.get(symbol).cloned()
}

/// Whether a token is an `Identifier` carrying the given spelling
/// (case-insensitive). Lookahead predicates that cannot use `check_keyword`
/// (it only inspects the current token) route their comparison through here
/// instead of repeating the match inline.
pub fn is_identifier_spelling(kind: &TokenKind, word: &str) -> bool {
    matches!(kind, TokenKind::Identifier(current) if current.eq_ignore_ascii_case(word))
}

/// Cross-check scanner output against the symbol table. Non-symbol kinds
/// always pass. Used as a `debug_assert` in the lexer so test builds catch
/// scanner/spec drift at the point of production.
pub fn check_symbol_token(kind: &TokenKind, lexeme: &str) -> bool {
    match SPEC_INDEX.symbol_lexemes.get(&discriminant(kind)) {
        None => true,
        Some(lexemes) => lexemes.iter().any(|symbol| *symbol == lexeme),
    }
}

impl TokenKindExt for TokenKind {
    fn is_keyword(&self) -> bool {
        match self {
            TokenKind::Identifier(_)
            | TokenKind::StringLiteral(_)
            | TokenKind::IntegerLiteral(_)
            | TokenKind::FloatLiteral(_)
            | TokenKind::BooleanLiteral(_)
            | TokenKind::Eof => false,
            other => SPEC_INDEX.reserved.contains(&discriminant(other)),
        }
    }

    fn is_literal(&self) -> bool {
        matches!(
            self,
            TokenKind::StringLiteral(_)
                | TokenKind::IntegerLiteral(_)
                | TokenKind::FloatLiteral(_)
                | TokenKind::BooleanLiteral(_)
        )
    }

    fn is_operator(&self) -> bool {
        SPEC_INDEX.operators.contains(&discriminant(self))
    }

    fn is_delimiter(&self) -> bool {
        SPEC_INDEX.punctuation.contains(&discriminant(self))
    }

    fn is_identifier(&self) -> bool {
        matches!(self, TokenKind::Identifier(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_spellings_are_unique() {
        let mut seen = HashSet::new();
        for entry in keyword_table() {
            assert!(
                seen.insert(entry.spelling),
                "duplicate keyword spelling '{}'",
                entry.spelling
            );
        }
    }

    #[test]
    fn lookup_reaches_every_table_row() {
        for entry in keyword_table() {
            assert_eq!(
                lookup_keyword_spelling(entry.spelling),
                Some(entry.kind.clone()),
                "lookup misses '{}'",
                entry.spelling
            );
        }
    }

    #[test]
    fn unit_map_covers_every_reserved_row() {
        for entry in keyword_table() {
            if entry.category() != KeywordCategory::Reserved {
                continue;
            }
            let spellings = unit_spellings_for(&entry.kind)
                .unwrap_or_else(|| panic!("unit map misses '{}'", entry.spelling));
            assert!(
                spellings.contains(&entry.spelling),
                "unit map for {:?} misses '{}'",
                entry.kind,
                entry.spelling
            );
        }
    }

    #[test]
    fn payload_rows_stay_out_of_check_keyword_map() {
        assert_eq!(
            KeywordEntry {
                spelling: "TRUE",
                kind: TokenKind::BooleanLiteral(true)
            }
            .category(),
            KeywordCategory::BooleanLiteral
        );
        assert!(unit_spellings_for(&TokenKind::BooleanLiteral(true)).is_none());
        assert!(unit_spellings_for(&TokenKind::BooleanLiteral(false)).is_none());
        assert!(unit_spellings_for(&TokenKind::Identifier("balance".to_string())).is_none());
    }

    #[test]
    fn contextual_keywords_are_identifiers_not_reserved() {
        let reserved: HashSet<&&str> = keyword_table()
            .iter()
            .map(|entry| &entry.spelling)
            .collect();
        let mut seen = HashSet::new();
        for word in CONTEXTUAL_KEYWORDS {
            assert!(
                seen.insert(*word),
                "duplicate contextual keyword '{word}'"
            );
            assert!(
                !reserved.contains(word),
                "'{word}' is reserved and must not be listed as contextual"
            );
        }
        for word in SOFT_KEYWORDS {
            assert!(
                CONTEXTUAL_KEYWORDS.contains(word),
                "soft keyword '{word}' must be part of the contextual list"
            );
        }
        for word in ["SELECT", "PARTITION", "COLLECT", "BINARY", "MULTIWAY"] {
            assert!(
                CONTEXTUAL_KEYWORDS.contains(&word),
                "ad-hoc recognition site keyword '{word}' must be listed as contextual"
            );
        }
    }

    #[test]
    fn symbol_table_has_no_duplicate_spellings() {
        let mut seen = HashSet::new();
        for entry in SYMBOL_SPEC {
            assert!(
                seen.insert(entry.symbol),
                "duplicate symbol spelling '{}'",
                entry.symbol
            );
        }
    }

    #[test]
    fn operator_punctuation_partition_covers_symbol_spec() {
        assert_eq!(
            operator_symbols().len() + punctuation_symbols().len(),
            SYMBOL_SPEC.len(),
            "operator/punctuation views must partition the symbol table"
        );
    }
}
